use crate::{AttachedConnector, Direction, ManagerConfig, ProjectStatus};
use anyhow::{anyhow, Error as AnyError, Result as AnyResult};
use chrono::{DateTime, NaiveDateTime, Utc};
use log::{debug, error};
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, ToSql,
};
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, fmt, fmt::Display};
use utoipa::ToSchema;

/// Project database API.
///
/// The API assumes that the caller holds a database lock, and therefore
/// doesn't use transactions (and hence doesn't need to deal with conflicts).
///
/// The database schema is defined in `create_db.sql`.
///
/// # Compilation queue
///
/// We use the `status` and `status_since` columns to maintain the compilation
/// queue.  A project is enqueued for compilation by setting its status to
/// [`ProjectStatus::Pending`].  The `status_since` column is set to the current
/// time, which determines the position of the project in the queue.
pub(crate) struct ProjectDB {
    dbclient: Connection,
}

/// Unique project id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct ProjectId(pub i64);
impl Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Unique configuration id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct ConfigId(pub i64);
impl Display for ConfigId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Unique pipeline id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct PipelineId(pub i64);
impl Display for PipelineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Unique connector id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct ConnectorId(pub i64);
impl Display for ConnectorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Unique attached connector id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct AttachedConnectorId(pub i64);
impl Display for AttachedConnectorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Version number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct Version(i64);
impl Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Version {
    fn increment(&self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug)]
pub(crate) enum DBError {
    UnknownProject(ProjectId),
    DuplicateProjectName(String),
    OutdatedProjectVersion(Version),
    UnknownConfig(ConfigId),
    UnknownPipeline(PipelineId),
    UnknownConnector(ConnectorId),
}

impl Display for DBError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DBError::UnknownProject(project_id) => write!(f, "Unknown project id '{project_id}'"),
            DBError::DuplicateProjectName(name) => {
                write!(f, "A project named '{name}' already exists")
            }
            DBError::OutdatedProjectVersion(version) => {
                write!(f, "Outdated project version '{version}'")
            }
            DBError::UnknownConfig(config_id) => {
                write!(f, "Unknown project config id '{config_id}'")
            }
            DBError::UnknownPipeline(pipeline_id) => {
                write!(f, "Unknown pipeline id '{pipeline_id}'")
            }
            DBError::UnknownConnector(connector_id) => {
                write!(f, "Unknown connector id '{connector_id}'")
            }
        }
    }
}

impl StdError for DBError {}

/// The database encodes project status using two columns: `status`, which has
/// type `string`, but acts as an enum, and `error`, only used if `status` is
/// one of `"sql_error"` or `"rust_error"`.
impl ProjectStatus {
    /// Decode `ProjectStatus` from the values of `error` and `status` columns.
    fn from_columns(status_string: Option<&str>, error_string: Option<String>) -> AnyResult<Self> {
        match status_string {
            None => Ok(Self::None),
            Some("success") => Ok(Self::Success),
            Some("pending") => Ok(Self::Pending),
            Some("compiling_sql") => Ok(Self::CompilingSql),
            Some("compiling_rust") => Ok(Self::CompilingRust),
            Some("sql_error") => {
                let error = error_string.unwrap_or_default();
                if let Ok(messages) = serde_json::from_str(&error) {
                    Ok(Self::SqlError(messages))
                } else {
                    error!("Expected valid json for SqlCompilerMessage but got {:?}, did you update the struct without adjusting the database?", error);
                    Ok(Self::SystemError(error))
                }
            }
            Some("rust_error") => Ok(Self::RustError(error_string.unwrap_or_default())),
            Some("system_error") => Ok(Self::SystemError(error_string.unwrap_or_default())),
            Some(status) => Err(AnyError::msg(format!("invalid status string '{status}'"))),
        }
    }
    fn to_columns(&self) -> (Option<String>, Option<String>) {
        match self {
            ProjectStatus::None => (None, None),
            ProjectStatus::Success => (Some("success".to_string()), None),
            ProjectStatus::Pending => (Some("pending".to_string()), None),
            ProjectStatus::CompilingSql => (Some("compiling_sql".to_string()), None),
            ProjectStatus::CompilingRust => (Some("compiling_rust".to_string()), None),
            ProjectStatus::SqlError(error) => {
                if let Ok(error_string) = serde_json::to_string(&error) {
                    (Some("sql_error".to_string()), Some(error_string))
                } else {
                    error!("Expected valid json for SqlError, but got {:?}", error);
                    (Some("sql_error".to_string()), None)
                }
            }
            ProjectStatus::RustError(error) => {
                (Some("rust_error".to_string()), Some(error.clone()))
            }
            ProjectStatus::SystemError(error) => {
                (Some("system_error".to_string()), Some(error.clone()))
            }
        }
    }
}

/// Project descriptor.
#[derive(Serialize, ToSchema, Debug)]
pub(crate) struct ProjectDescr {
    /// Unique project id.
    pub project_id: ProjectId,
    /// Project name (doesn't have to be unique).
    pub name: String,
    /// Project description.
    pub description: String,
    /// Project version, incremented every time project code is modified.
    pub version: Version,
    /// Project compilation status.
    pub status: ProjectStatus,
    /// A JSON description of the SQL tables and view declarations including
    /// field names and types.
    ///
    /// The schema is set/updated whenever the `status` field reaches >=
    /// `ProjectStatus::CompilingRust`.
    ///
    /// # Example
    ///
    /// The given SQL program:
    ///
    /// ```no-run
    /// CREATE TABLE USERS ( name varchar );
    /// CREATE VIEW OUTPUT_USERS as SELECT * FROM USERS;
    /// ```
    ///
    /// Would lead the following JSON string in `schema`:
    ///
    /// ```no-run
    /// {
    ///   "inputs": [{
    ///       "name": "USERS",
    ///       "fields": [{ "name": "NAME", "type": "VARCHAR", "nullable": true }]
    ///     }],
    ///   "outputs": [{
    ///       "name": "OUTPUT_USERS",
    ///       "fields": [{ "name": "NAME", "type": "VARCHAR", "nullable": true }]
    ///     }]
    /// }
    /// ```
    pub schema: Option<String>,
}

/// Project configuration descriptor.
#[derive(Serialize, ToSchema, Debug)]
pub(crate) struct ConfigDescr {
    pub config_id: ConfigId,
    pub project_id: Option<ProjectId>,
    pub pipeline: Option<PipelineDescr>,
    pub version: Version,
    pub name: String,
    pub description: String,
    pub config: String,
    pub attached_connectors: Vec<AttachedConnector>,
}

/// Pipeline descriptor.
#[derive(Serialize, ToSchema, Debug)]
pub(crate) struct PipelineDescr {
    pub pipeline_id: PipelineId,
    pub config_id: ConfigId,
    pub port: u16,
    pub killed: bool,
    pub created: DateTime<Utc>,
}

/// Type of new data connector.
#[derive(Serialize, Deserialize, ToSchema, Debug, Copy, Clone)]
pub enum ConnectorType {
    KafkaIn = 0,
    KafkaOut = 1,
    File = 2,
}

impl ToSql for ConnectorType {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput> {
        Ok(ToSqlOutput::from(*self as i64))
    }
}

impl Into<Direction> for ConnectorType {
    fn into(self) -> Direction {
        match self {
            ConnectorType::KafkaIn => Direction::Input,
            ConnectorType::KafkaOut => Direction::Output,
            ConnectorType::File => Direction::InputOutput,
        }
    }
}

impl FromSql for ConnectorType {
    fn column_result(value: ValueRef) -> FromSqlResult<Self> {
        match value.as_i64() {
            Ok(0) => Ok(ConnectorType::KafkaIn),
            Ok(1) => Ok(ConnectorType::KafkaOut),
            Ok(2) => Ok(ConnectorType::File),
            Ok(idx) => Err(FromSqlError::OutOfRange(idx)),
            Err(_) => Err(FromSqlError::InvalidType),
        }
    }
}

/// Connector descriptor.
#[derive(Serialize, ToSchema, Debug)]
pub(crate) struct ConnectorDescr {
    pub connector_id: ConnectorId,
    pub name: String,
    pub description: String,
    pub typ: ConnectorType,
    pub config: String,
    pub direction: Direction,
}

impl ProjectDB {
    /// Connect to the project database.
    ///
    /// `config.pg_connection_string` must specify location of a project
    /// database created with `create_db.sql` along with access credentials.
    pub(crate) fn connect(config: &ManagerConfig) -> AnyResult<Self> {
        unsafe {
            rusqlite::trace::config_log(Some(|errcode, msg| debug!("sqlite: {msg}:{errcode}")))?
        };
        let dbclient = Connection::open(config.database_file_path())?;

        dbclient.execute(
            r#"
CREATE TABLE IF NOT EXISTS project (
    id integer PRIMARY KEY AUTOINCREMENT,
    version integer,
    name varchar UNIQUE,
    description varchar,
    code varchar,
    schema varchar,
    status varchar,
    error varchar,
    status_since integer)"#,
            (),
        )?;

        dbclient.execute(
            r#"
CREATE TABLE IF NOT EXISTS pipeline (
    id integer PRIMARY KEY AUTOINCREMENT,
    config_id integer,
    config_version integer,
    -- TODO: add 'host' field when we support remote pipelines.
    port integer,
    killed bool NOT NULL,
    created integer
)"#,
            (),
        )?;

        dbclient.execute(
            r#"
CREATE TABLE IF NOT EXISTS project_config (
    id integer PRIMARY KEY AUTOINCREMENT,
    pipeline_id integer,
    project_id integer,
    version integer,
    name varchar,
    description varchar,
    config varchar,
    FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
    FOREIGN KEY (pipeline_id) REFERENCES pipeline(id) ON DELETE SET NULL
)"#,
            (),
        )?;

        dbclient.execute(
            r#"
CREATE TABLE IF NOT EXISTS attached_connector (
    id integer PRIMARY KEY AUTOINCREMENT,
    uuid varchar UNIQUE,
    config_id integer,
    connector_id integer,
    config varchar,
    is_input bool,
    FOREIGN KEY (config_id) REFERENCES project_config(id) ON DELETE CASCADE
    FOREIGN KEY (connector_id) REFERENCES connector(id) ON DELETE CASCADE)"#,
            (),
        )?;

        dbclient.execute(
            r#"
CREATE TABLE IF NOT EXISTS connector (
    id integer PRIMARY KEY AUTOINCREMENT,
    version integer,
    name varchar,
    description varchar,
    typ integer,
    config text)"#,
            (),
        )?;

        if let Some(initial_sql_file) = &config.initial_sql {
            if let Ok(initial_sql) = std::fs::read_to_string(initial_sql_file) {
                dbclient.execute(&initial_sql, ())?;
            } else {
                log::warn!("initial SQL file '{}' does not exist", initial_sql_file);
            }
        }

        Ok(Self { dbclient })
    }

    /// Reset everything that is set through compilation of the project.
    ///
    /// - Set status to `ProjectStatus::None` after server restart.
    /// - Reset `schema` to None.
    pub(crate) fn reset_project_status(&self) -> AnyResult<()> {
        self.dbclient.execute(
            "UPDATE project SET status = NULL, error = NULL, schema = NULL",
            (),
        )?;

        Ok(())
    }

    /// Retrieve project list from the DB.
    pub(crate) async fn list_projects(&self) -> AnyResult<Vec<ProjectDescr>> {
        let mut statement = self
            .dbclient
            .prepare("SELECT id, name, description, version, status, error, schema FROM project")?;
        let mut rows = statement.query([])?;

        let mut result = Vec::new();

        while let Some(row) = rows.next()? {
            let status: Option<String> = row.get(4)?;
            let error: Option<String> = row.get(5)?;
            let status = ProjectStatus::from_columns(status.as_deref(), error)?;
            let schema: Option<String> = row.get(6)?;

            result.push(ProjectDescr {
                project_id: ProjectId(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                version: Version(row.get(3)?),
                schema,
                status,
            });
        }

        Ok(result)
    }

    /// Retrieve code of the specified project along with the project's
    /// meta-data.
    pub(crate) fn project_code(&self, project_id: ProjectId) -> AnyResult<(ProjectDescr, String)> {
        let mut statement = self.dbclient.prepare(
            "SELECT name, description, version, status, error, code, schema FROM project WHERE id = $1",
        )?;
        let mut rows = statement.query([&project_id.0])?;

        if let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let description: String = row.get(1)?;
            let version: Version = Version(row.get(2)?);
            let status: Option<String> = row.get(3)?;
            let error: Option<String> = row.get(4)?;
            let code: String = row.get(5)?;
            let schema: Option<String> = row.get(6)?;

            let status = ProjectStatus::from_columns(status.as_deref(), error)?;

            Ok((
                ProjectDescr {
                    project_id,
                    name,
                    description,
                    version,
                    status,
                    schema,
                },
                code,
            ))
        } else {
            Err(DBError::UnknownProject(project_id).into())
        }
    }

    /// Helper to convert rusqlite error into a `DBError::DuplicateProjectName`
    /// if the underlying low-level error thrown by the database matches.
    fn maybe_duplicate_project_name_err(e: rusqlite::Error, project_name: &str) -> AnyError {
        if let rusqlite::Error::SqliteFailure(sqlite_failure, Some(msg)) = &e {
            if sqlite_failure
                == (&rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::ConstraintViolation,
                    extended_code: rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE,
                })
                && msg.as_str() == "UNIQUE constraint failed: project.name"
            {
                anyhow!(DBError::DuplicateProjectName(project_name.to_string()))
            } else {
                anyhow!(e)
            }
        } else {
            anyhow!(e)
        }
    }

    /// Create a new project.
    pub(crate) fn new_project(
        &self,
        project_name: &str,
        project_description: &str,
        project_code: &str,
    ) -> AnyResult<(ProjectId, Version)> {
        debug!("new_project {project_name} {project_description} {project_code}");
        self.dbclient
            .execute(
                "INSERT INTO project (version, name, description, code, status_since) VALUES(1, $1, $2, $3, unixepoch('now'))",
                (&project_name, &project_description, &project_code),
            ).map_err(|e| ProjectDB::maybe_duplicate_project_name_err(e, project_name))?;

        let id = self
            .dbclient
            .query_row("SELECT last_insert_rowid()", (), |row| {
                Ok(ProjectId(row.get(0)?))
            })?;

        Ok((id, Version(1)))
    }

    /// Update project name and, optionally, code.
    pub(crate) fn update_project(
        &mut self,
        project_id: ProjectId,
        project_name: &str,
        project_description: &str,
        project_code: &Option<String>,
    ) -> AnyResult<Version> {
        let (mut version, old_code): (Version, String) = self
            .dbclient
            .query_row(
                "SELECT version, code FROM project where id = $1",
                [&project_id.0],
                |row| Ok((Version(row.get(0)?), row.get(1)?)),
            )
            .map_err(|_| DBError::UnknownProject(project_id))?;

        match project_code {
            Some(code) if &old_code != code => {
                // Only increment `version` if new code actually differs from the
                // current version.
                version = version.increment();
                self.dbclient
                    .execute(
                        "UPDATE project SET version = $1, name = $2, description = $3, code = $4, status = NULL, error = NULL WHERE id = $5",
                        (&version.0, &project_name, &project_description, code, &project_id.0),
                    ).map_err(|e| ProjectDB::maybe_duplicate_project_name_err(e, project_name))?;
            }
            _ => {
                self.dbclient
                    .execute(
                        "UPDATE project SET name = $1, description = $2 WHERE id = $3",
                        (&project_name, &project_description, &project_id.0),
                    )
                    .map_err(|e| ProjectDB::maybe_duplicate_project_name_err(e, project_name))?;
            }
        }

        Ok(version)
    }

    /// Retrieve project descriptor.
    ///
    /// Returns `None` if `project_id` is not found in the database.
    pub(crate) fn get_project_if_exists(
        &self,
        project_id: ProjectId,
    ) -> AnyResult<Option<ProjectDescr>> {
        let mut statement = self.dbclient.prepare(
            "SELECT name, description, version, status, error, schema FROM project WHERE id = $1",
        )?;
        let mut rows = statement.query([&project_id.0])?;

        if let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let description: String = row.get(1)?;
            let version: Version = Version(row.get(2)?);
            let status: Option<String> = row.get(3)?;
            let error: Option<String> = row.get(4)?;
            let schema: Option<String> = row.get(5)?;

            let status = ProjectStatus::from_columns(status.as_deref(), error)?;

            Ok(Some(ProjectDescr {
                project_id,
                name,
                description,
                version,
                status,
                schema,
            }))
        } else {
            Ok(None)
        }
    }

    /// Lookup project by name
    pub(crate) fn lookup_project(&self, project_name: &str) -> AnyResult<Option<ProjectDescr>> {
        let mut statement = self.dbclient.prepare(
            "SELECT id, description, version, status, error, schema FROM project WHERE name = $1",
        )?;
        let mut rows = statement.query([project_name])?;

        if let Some(row) = rows.next()? {
            let project_id: ProjectId = ProjectId(row.get(0)?);
            let description: String = row.get(1)?;
            let version: Version = Version(row.get(2)?);
            let status: Option<String> = row.get(3)?;
            let error: Option<String> = row.get(4)?;
            let schema: Option<String> = row.get(5)?;

            let status = ProjectStatus::from_columns(status.as_deref(), error)?;

            Ok(Some(ProjectDescr {
                project_id,
                name: project_name.to_string(),
                description,
                version,
                status,
                schema,
            }))
        } else {
            Ok(None)
        }
    }

    /// Retrieve project descriptor.
    ///
    /// Returns a `DBError:UnknownProject` error if `project_id` is not found in
    /// the database.
    pub(crate) fn get_project(&self, project_id: ProjectId) -> AnyResult<ProjectDescr> {
        self.get_project_if_exists(project_id)?
            .ok_or_else(|| anyhow!(DBError::UnknownProject(project_id)))
    }

    /// Validate project version and retrieve project descriptor.
    ///
    /// Returns `DBError::UnknownProject` if `project_id` is not found in the
    /// database. Returns `DBError::OutdatedProjectVersion` if the current
    /// project version differs from `expected_version`.
    pub(crate) fn get_project_guarded(
        &self,
        project_id: ProjectId,
        expected_version: Version,
    ) -> AnyResult<ProjectDescr> {
        let descr = self.get_project(project_id)?;
        if descr.version != expected_version {
            return Err(anyhow!(DBError::OutdatedProjectVersion(expected_version)));
        }

        Ok(descr)
    }

    /// Update project status.
    ///
    /// # Note
    /// - Doesn't check that the project exists.
    /// - Resets schema to null.
    fn set_project_status(&self, project_id: ProjectId, status: ProjectStatus) -> AnyResult<()> {
        let (status, error) = status.to_columns();

        self.dbclient
            .execute(
                "UPDATE project SET status = $1, error = $2, schema = null, status_since = unixepoch('now') WHERE id = $3",
                (&status, &error, &project_id.0),
            )?;

        Ok(())
    }

    /// Update project status after a version check.
    ///
    /// Updates project status to `status` if the current project version in the
    /// database matches `expected_version`.
    ///
    /// # Note
    /// This intentionally does not throw an error if there is a project version
    /// mismatch and instead does just not update. It's used by the compiler to
    /// update status and in case there is a newer version it is expected that
    /// the compiler just picks up and runs the next job.
    pub(crate) fn set_project_status_guarded(
        &mut self,
        project_id: ProjectId,
        expected_version: Version,
        status: ProjectStatus,
    ) -> AnyResult<()> {
        let (status, error) = status.to_columns();

        let descr = self.get_project(project_id)?;
        if descr.version == expected_version {
            self.dbclient
                .execute(
                    "UPDATE project SET status = $1, error = $2, status_since = unixepoch('now') WHERE id = $3",
                    (&status, &error, &project_id.0),
                )?;
        }

        Ok(())
    }

    /// Update project schema.
    ///
    /// # Note
    /// This should be called after the SQL compilation succeeded, e.g., in the
    /// same transaction that sets status to  [`ProjectStatus::CompilingRust`].
    pub(crate) fn set_project_schema(
        &mut self,
        project_id: ProjectId,
        schema: String,
    ) -> AnyResult<()> {
        self.dbclient.execute(
            "UPDATE project SET schema = $1 WHERE id = $2",
            (&schema, &project_id.0),
        )?;

        Ok(())
    }

    /// Queue project for compilation by setting its status to
    /// [`ProjectStatus::Pending`].
    ///
    /// Change project status to [`ProjectStatus::Pending`].
    pub(crate) fn set_project_pending(
        &self,
        project_id: ProjectId,
        expected_version: Version,
    ) -> AnyResult<()> {
        let descr = self.get_project_guarded(project_id, expected_version)?;

        // Do nothing if the project is already pending (we don't want to bump its
        // `status_since` field, which would move it to the end of the queue) or
        // if compilation is alread in progress.
        if descr.status == ProjectStatus::Pending || descr.status.is_compiling() {
            return Ok(());
        }

        self.set_project_status(project_id, ProjectStatus::Pending)?;

        Ok(())
    }

    /// Cancel compilation request.
    ///
    /// Cancels compilation request if the project is pending in the queue
    /// or already being compiled.
    pub(crate) fn cancel_project(
        &self,
        project_id: ProjectId,
        expected_version: Version,
    ) -> AnyResult<()> {
        let descr = self.get_project_guarded(project_id, expected_version)?;

        if descr.status != ProjectStatus::Pending || !descr.status.is_compiling() {
            return Ok(());
        }

        self.set_project_status(project_id, ProjectStatus::None)?;

        Ok(())
    }

    /// Delete project from the database.
    ///
    /// This will delete all project configs and pipelines.
    pub(crate) fn delete_project(&self, project_id: ProjectId) -> AnyResult<()> {
        let num_deleted = self
            .dbclient
            .execute("DELETE FROM project WHERE id = $1", [&project_id.0])?;

        if num_deleted > 0 {
            Ok(())
        } else {
            Err(anyhow!(DBError::UnknownProject(project_id)))
        }
    }

    /// Retrieves the first pending project from the queue.
    ///
    /// Returns a pending project with the most recent `status_since` or `None`
    /// if there are no pending projects in the DB.
    pub(crate) fn next_job(&self) -> AnyResult<Option<(ProjectId, Version)>> {
        // Find the oldest pending project.
        let mut statement = self
            .dbclient
            .prepare("SELECT id, version FROM project WHERE status = 'pending' AND status_since = (SELECT min(status_since) FROM project WHERE status = 'pending')")?;
        let mut rows = statement.query([])?;

        if let Some(row) = rows.next()? {
            let project_id: ProjectId = ProjectId(row.get(0)?);
            let version: Version = Version(row.get(1)?);

            Ok(Some((project_id, version)))
        } else {
            Ok(None)
        }
    }

    /// List configs associated with `project_id`.
    pub(crate) fn list_configs(&self) -> AnyResult<Vec<ConfigDescr>> {
        let mut statement = self.dbclient.prepare(
            "SELECT id, version, name, description, config, pipeline_id, project_id FROM project_config",
        )?;
        let mut rows = statement.query([])?;
        let mut result = Vec::new();

        while let Some(row) = rows.next()? {
            let config_id = ConfigId(row.get(0)?);
            let project_id = row.get(6).map(|id: Option<i64>| id.map(ProjectId))?;
            let attached_connectors = self.get_attached_connectors(config_id)?;
            let pipeline =
                if let Some(pipeline_id) = row.get(5).map(|id: Option<i64>| id.map(PipelineId))? {
                    log::info!("pipeline_id: {:?}", pipeline_id);
                    let pp = Some(self.get_pipeline(pipeline_id)?);
                    log::info!("pp: {:?}", pp);
                    pp
                } else {
                    None
                };

            result.push(ConfigDescr {
                config_id,
                version: Version(row.get(1)?),
                name: row.get(2)?,
                description: row.get(3)?,
                config: row.get(4)?,
                pipeline,
                project_id,
                attached_connectors,
            });
        }

        Ok(result)
    }

    /// Retrieve project config.
    pub(crate) fn get_config(&self, config_id: ConfigId) -> AnyResult<ConfigDescr> {
        log::info!("get_config({:?})", config_id.0);
        let mut statement = self.dbclient.prepare(
            "SELECT id, version, name, description, config, pipeline_id, project_id FROM project_config WHERE id = $1",
        )?;
        let mut rows = statement.query([&config_id.0])?;

        if let Some(row) = rows.next()? {
            let project_id = row.get(6).map(|id: Option<i64>| id.map(ProjectId))?;
            let attached_connectors = self.get_attached_connectors(config_id)?;
            let pipeline =
                if let Some(pipeline_id) = row.get(5).map(|id: Option<i64>| id.map(PipelineId))? {
                    Some(self.get_pipeline(pipeline_id)?)
                } else {
                    None
                };

            Ok(ConfigDescr {
                config_id,
                version: Version(row.get(1)?),
                name: row.get(2)?,
                description: row.get(3)?,
                config: row.get(4)?,
                pipeline,
                project_id,
                attached_connectors,
            })
        } else {
            Err(anyhow!(DBError::UnknownConfig(config_id)))
        }
    }

    /// Create a new project config.
    pub(crate) fn new_config(
        &self,
        project_id: Option<ProjectId>,
        config_name: &str,
        config_description: &str,
        config: &str,
    ) -> AnyResult<(ConfigId, Version)> {
        if let Some(pid) = project_id {
            // Check that the project exists, so we return correct error status
            // instead of Internal Server Error due to the next query failing.
            let _descr = self.get_project(pid)?;
        }

        self.dbclient.execute(
            "INSERT INTO project_config (project_id, version, name, description, config) VALUES($1, 1, $2, $3, $4)",
            (&project_id.map(|pid| pid.0), &config_name, &config_description, &config),
        )?;

        let id = self
            .dbclient
            .query_row("SELECT last_insert_rowid()", (), |row| {
                Ok(ConfigId(row.get(0)?))
            })?;

        Ok((id, Version(1)))
    }

    /// Add pipeline to the config.
    pub(crate) fn add_pipeline_to_config(
        &self,
        config_id: ConfigId,
        pipeline_id: PipelineId,
    ) -> AnyResult<()> {
        self.dbclient.execute(
            "UPDATE project_config SET pipeline_id = $1 WHERE id = $2",
            [&pipeline_id.0, &config_id.0],
        )?;

        Ok(())
    }

    /// Update existing project config.
    ///
    /// Update config name, description, project id and, optionally, YAML, and
    /// connector configs.
    pub(crate) fn update_config(
        &mut self,
        config_id: ConfigId,
        project_id: Option<ProjectId>,
        config_name: &str,
        config_description: &str,
        config: &Option<String>,
        connectors: &Option<Vec<AttachedConnector>>,
    ) -> AnyResult<Version> {
        log::info!(
            "Updating config {} {} {} {} {:?} {:?}",
            config_id.0,
            project_id.map(|pid| pid.0).unwrap_or(-1),
            config_name,
            config_description,
            config,
            connectors
        );
        let descr = self.get_config(config_id)?;
        let config = config.clone().unwrap_or(descr.config);

        if let Some(connectors) = connectors {
            // Delete all existing attached connectors.
            self.dbclient.execute(
                "DELETE FROM attached_connector WHERE config_id = $1",
                [&config_id.0],
            )?;

            // Rewrite the new set of connectors.
            for ac in connectors {
                self.attach_connector(config_id, ac)?;
            }
        }

        let version = descr.version.increment();
        self.dbclient.execute(
            "UPDATE project_config SET version = $1, name = $2, description = $3, config = $4, project_id = $5 WHERE id = $6",
            (&version.0, &config_name, &config_description, &config, project_id.map(|p| p.0), &config_id.0),
        )?;

        Ok(version)
    }

    /// Delete project config.
    pub(crate) fn delete_config(&self, config_id: ConfigId) -> AnyResult<()> {
        let num_deleted = self
            .dbclient
            .execute("DELETE FROM project_config WHERE id = $1", [&config_id.0])?;

        if num_deleted > 0 {
            Ok(())
        } else {
            Err(anyhow!(DBError::UnknownConfig(config_id)))
        }
    }

    fn get_attached_connectors(&self, config_id: ConfigId) -> AnyResult<Vec<AttachedConnector>> {
        let mut statement = self.dbclient.prepare(
            "SELECT uuid, connector_id, config, is_input FROM attached_connector WHERE config_id = $1",
        )?;
        let mut rows = statement.query([config_id.0])?;
        let mut result = Vec::new();

        while let Some(row) = rows.next()? {
            let direction = if row.get(3)? {
                Direction::Input
            } else {
                Direction::Output
            };

            result.push(AttachedConnector {
                uuid: row.get(0)?,
                connector_id: ConnectorId(row.get(1)?),
                config: row.get(2)?,
                direction,
            });
        }

        Ok(result)
    }

    fn attach_connector(&mut self, config_id: ConfigId, ac: &AttachedConnector) -> AnyResult<()> {
        let _descr = self.get_config(config_id)?;
        let _descr = self.get_connector(ac.connector_id)?;
        let is_input = ac.direction == Direction::Input;

        self.dbclient.execute(
            "INSERT INTO attached_connector (uuid, config_id, connector_id, is_input, config) VALUES($1, $2, $3, $4, $5)",
            (&ac.uuid, &config_id.0, &ac.connector_id.0, is_input, &ac.config),
        )?;

        Ok(())
    }

    /// Insert a new record to the `pipeline` table.
    pub(crate) fn new_pipeline(
        &self,
        config_id: ConfigId,
        config_version: Version,
    ) -> AnyResult<PipelineId> {
        self.dbclient
            .execute(
                "INSERT INTO pipeline (config_id, config_version, killed, created) VALUES($1, $2, false, unixepoch('now'))",
                (&config_id.0, &config_version.0),
            )?;

        let id = self
            .dbclient
            .query_row("SELECT last_insert_rowid()", (), |row| {
                Ok(PipelineId(row.get(0)?))
            })?;

        Ok(id)
    }

    pub(crate) fn pipeline_set_port(&self, pipeline_id: PipelineId, port: u16) -> AnyResult<()> {
        self.dbclient.execute(
            "UPDATE pipeline SET port = $1 where id = $2",
            (&port, &pipeline_id.0),
        )?;

        Ok(())
    }

    /// Read pipeline status.
    ///
    /// Returns pipeline port number and `killed` flag.
    pub(crate) async fn pipeline_status(&self, pipeline_id: PipelineId) -> AnyResult<(u16, bool)> {
        let (port, killed): (i32, bool) = self
            .dbclient
            .query_row(
                "SELECT port, killed FROM pipeline WHERE id = $1",
                [&pipeline_id.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| anyhow!(DBError::UnknownPipeline(pipeline_id)))?;

        Ok((port as u16, killed))
    }

    /// Set `killed` flag to `true`.
    pub(crate) fn set_pipeline_killed(&self, pipeline_id: PipelineId) -> AnyResult<bool> {
        let num_updated = self.dbclient.execute(
            "UPDATE pipeline SET killed=true WHERE id = $1",
            [&pipeline_id.0],
        )?;

        Ok(num_updated > 0)
    }

    /// Delete `pipeline` from the DB.
    pub(crate) fn delete_pipeline(&self, pipeline_id: PipelineId) -> AnyResult<bool> {
        let num_deleted = self
            .dbclient
            .execute("DELETE FROM pipeline WHERE id = $1", [&pipeline_id.0])?;

        Ok(num_deleted > 0)
    }

    /// Retrieve project config.
    pub(crate) fn get_pipeline(&self, pipeline_id: PipelineId) -> AnyResult<PipelineDescr> {
        let mut statement = self.dbclient.prepare(
            "SELECT id, config_id, config_version, port, killed, created FROM pipeline WHERE id = $1",
        )?;
        let mut rows = statement.query([&pipeline_id.0])?;

        if let Some(row) = rows.next()? {
            let created_secs: i64 = row.get(5)?;
            let created_naive = NaiveDateTime::from_timestamp_millis(created_secs * 1000)
                .ok_or_else(|| {
                    AnyError::msg(format!(
                        "Invalid timestamp in 'pipeline.created' column: {created_secs}"
                    ))
                })?;

            Ok(PipelineDescr {
                pipeline_id: PipelineId(row.get(0)?),
                config_id: ConfigId(row.get(1)?),
                port: row.get::<_, i32>(3)? as u16,
                killed: row.get(4)?,
                created: DateTime::<Utc>::from_utc(created_naive, Utc),
            })
        } else {
            Err(anyhow!(DBError::UnknownPipeline(pipeline_id)))
        }
    }

    /// List pipelines.
    pub(crate) fn list_pipelines(&self) -> AnyResult<Vec<PipelineDescr>> {
        let mut statement = self
            .dbclient
            .prepare("SELECT id, config_id, config_version, port, killed, created FROM pipeline")?;
        let mut rows = statement.query([])?;

        let mut result = Vec::new();

        while let Some(row) = rows.next()? {
            let created_secs: i64 = row.get(5)?;
            let created_naive = NaiveDateTime::from_timestamp_millis(created_secs * 1000)
                .ok_or_else(|| {
                    AnyError::msg(format!(
                        "Invalid timestamp in 'pipeline.created' column: {created_secs}"
                    ))
                })?;

            result.push(PipelineDescr {
                pipeline_id: PipelineId(row.get(0)?),
                config_id: ConfigId(row.get(1)?),
                port: row.get::<_, i32>(3)? as u16,
                killed: row.get(4)?,
                created: DateTime::<Utc>::from_utc(created_naive, Utc),
            });
        }

        Ok(result)
    }

    /// Create a new connector.
    pub(crate) fn new_connector(
        &self,
        name: &str,
        description: &str,
        typ: ConnectorType,
        config: &str,
    ) -> AnyResult<ConnectorId> {
        debug!("new_connector {name} {description} {config}");
        self.dbclient.execute(
            "INSERT INTO connector (name, description, typ, config) VALUES($1, $2, $3, $4)",
            (&name, &description, typ, &config),
        )?;

        let id = self
            .dbclient
            .query_row("SELECT last_insert_rowid()", (), |row| {
                Ok(ConnectorId(row.get(0)?))
            })?;

        Ok(id)
    }

    /// Retrieve connectors list from the DB.
    pub(crate) async fn list_connectors(&self) -> AnyResult<Vec<ConnectorDescr>> {
        let mut statement = self
            .dbclient
            .prepare("SELECT id, name, description, typ, config FROM connector")?;
        let mut rows = statement.query([])?;
        let mut result = Vec::new();

        while let Some(row) = rows.next()? {
            let typ = row.get(3)?;
            result.push(ConnectorDescr {
                connector_id: ConnectorId(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                typ,
                direction: typ.into(),
                config: row.get(4)?,
            });
        }

        Ok(result)
    }

    /// Retrieve connector descriptor.
    pub(crate) fn get_connector(&self, connector_id: ConnectorId) -> AnyResult<ConnectorDescr> {
        let mut statement = self
            .dbclient
            .prepare("SELECT name, description, typ, config FROM connector WHERE id = $1")?;
        let mut rows = statement.query([&connector_id.0])?;

        if let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let description: String = row.get(1)?;
            let typ: ConnectorType = row.get(2)?;
            let config: String = row.get(3)?;

            Ok(ConnectorDescr {
                connector_id,
                name,
                description,
                typ,
                direction: typ.into(),
                config,
            })
        } else {
            Err(anyhow!(DBError::UnknownConnector(connector_id)))
        }
    }

    /// Update existing connector config.
    ///
    /// Update connector name and, optionally, YAML.
    pub(crate) fn update_connector(
        &mut self,
        connector_id: ConnectorId,
        connector_name: &str,
        description: &str,
        config: &Option<String>,
    ) -> AnyResult<()> {
        let descr = self.get_connector(connector_id)?;
        let config = config.clone().unwrap_or(descr.config);

        self.dbclient.execute(
            "UPDATE connector SET name = $1, description = $2, config = $3 WHERE id = $4",
            (
                &connector_name,
                &description,
                &config.as_str(),
                &connector_id.0,
            ),
        )?;

        Ok(())
    }

    /// Delete connector from the database.
    ///
    /// This will delete all connector configs and pipelines.
    pub(crate) fn delete_connector(&self, connector_id: ConnectorId) -> AnyResult<()> {
        let num_deleted = self
            .dbclient
            .execute("DELETE FROM connector WHERE id = $1", [&connector_id.0])?;

        if num_deleted > 0 {
            Ok(())
        } else {
            Err(anyhow!(DBError::UnknownConnector(connector_id)))
        }
    }
}
