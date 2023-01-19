use crate::{PipelineId, ProjectId};
use anyhow::{Error as AnyError, Result as AnyResult};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::fs::{canonicalize, create_dir_all};

const fn default_server_port() -> u16 {
    8080
}

fn default_pg_connection_string() -> String {
    "host=localhost user=dbsp".to_string()
}

fn default_working_directory() -> String {
    ".".to_string()
}

/// Pipeline manager configuration read from a YAML config file.
#[derive(Deserialize, Clone)]
pub(crate) struct ManagerConfig {
    /// Port number for the HTTP service, defaults to 8080.
    #[serde(default = "default_server_port")]
    pub port: u16,

    /// Postgres database connection string.
    ///
    /// See `tokio-postgres` crate documentation for supported formats.
    /// Defaults to "host=localhost user=dbsp".
    #[serde(default = "default_pg_connection_string")]
    pub pg_connection_string: String,

    /// Directory where the manager stores its filesystem state:
    /// generated Rust crates, pipeline logs, Prometheus config file,
    /// etc.  The server will create a subdirectory named `pipeline_data`
    /// under `working_directory`.
    #[serde(default = "default_working_directory")]
    pub working_directory: String,

    /// Location of the SQL-to-DBSP compiler.
    pub sql_compiler_home: String,

    /// Override DBSP dependencies in generated Rust crates.
    ///
    /// By default the Rust crates generated by the SQL compiler
    /// depend on github versions of DBSP crates
    /// (`dbsp`, `dbsp_adapters`).  This configuration options
    /// modifies the dependency to point to a source tree in the
    /// local file system.
    pub dbsp_override_path: Option<String>,

    /// When specified, the server will serve static web contents
    /// from this directory; otherwise it will use the static contents
    /// embedded in the manager executable.
    pub static_html: Option<String>,

    /// When `true`, the pipeline manager will start Prometheus
    /// and configure it to monitor the directory where the manager
    /// writes Prometheus config files for all pipelines.
    ///
    /// The default is `false`.
    #[serde(default)]
    pub with_prometheus: bool,

    /// Compile pipelines in debug mode.
    ///
    /// The default is `false`.
    #[serde(default)]
    pub debug: bool,
}

impl ManagerConfig {
    /// Convert all directory paths in the `self` to absolute paths.
    ///
    /// Converts `working_directory` `sql_compiler_home`,
    /// `dbsp_override_path`, and `static_html` fields to absolute paths;
    /// fails if any of the paths doesn't exist or isn't readable.
    pub(crate) async fn canonicalize(self) -> AnyResult<Self> {
        let mut result = self.clone();
        create_dir_all(&result.working_directory)
            .await
            .map_err(|e| {
                AnyError::msg(format!(
                    "unable to create or open working directry '{}': {e}",
                    result.working_directory
                ))
            })?;

        result.working_directory = canonicalize(&result.working_directory)
            .await
            .map_err(|e| {
                AnyError::msg(format!(
                    "error canonicalizing working directory path '{}': {e}",
                    result.working_directory
                ))
            })?
            .to_string_lossy()
            .into_owned();
        result.sql_compiler_home = canonicalize(&result.sql_compiler_home)
            .await
            .map_err(|e| {
                AnyError::msg(format!(
                    "failed to access SQL compiler home '{}': {e}",
                    result.sql_compiler_home
                ))
            })?
            .to_string_lossy()
            .into_owned();

        if let Some(path) = result.dbsp_override_path.as_mut() {
            *path = canonicalize(&path)
                .await
                .map_err(|e| {
                    AnyError::msg(format!(
                        "failed to access dbsp override directory '{path}': {e}"
                    ))
                })?
                .to_string_lossy()
                .into_owned();
        }

        if let Some(path) = result.static_html.as_mut() {
            *path = canonicalize(&path)
                .await
                .map_err(|e| AnyError::msg(format!("failed to access '{path}': {e}")))?
                .to_string_lossy()
                .into_owned();
        }

        Ok(result)
    }

    /// Crate name for a project.
    ///
    /// Note: we rely on the project id and not name, so projects can
    /// be renamed without recompiling.
    pub(crate) fn crate_name(project_id: ProjectId) -> String {
        format!("project{project_id}")
    }

    /// Directory where the manager maintains the generated cargo workspace.
    pub(crate) fn workspace_dir(&self) -> PathBuf {
        Path::new(&self.working_directory).join("cargo_workspace")
    }

    /// Directory where the manager generates Rust crate for the project.
    pub(crate) fn project_dir(&self, project_id: ProjectId) -> PathBuf {
        self.workspace_dir().join(Self::crate_name(project_id))
    }

    /// File name where the manager stores the SQL code of the project.
    pub(crate) fn sql_file_path(&self, project_id: ProjectId) -> PathBuf {
        self.project_dir(project_id).join("project.sql")
    }

    /// SQL compiler executable.
    pub(crate) fn sql_compiler_path(&self) -> PathBuf {
        Path::new(&self.sql_compiler_home)
            .join("SQL-compiler")
            .join("sql-to-dbsp")
    }

    /// Location of the Rust libraries that ship with the SQL compiler.
    pub(crate) fn sql_lib_path(&self) -> PathBuf {
        Path::new(&self.sql_compiler_home).join("lib")
    }

    /// File to redirect compiler's stderr stream.
    pub(crate) fn compiler_stderr_path(&self, project_id: ProjectId) -> PathBuf {
        self.project_dir(project_id).join("err.log")
    }

    /// File to redirect compiler's stdout stream.
    pub(crate) fn compiler_stdout_path(&self, project_id: ProjectId) -> PathBuf {
        self.project_dir(project_id).join("out.log")
    }

    /// Path to the generated `main.rs` for the project.
    pub(crate) fn rust_program_path(&self, project_id: ProjectId) -> PathBuf {
        self.project_dir(project_id).join("src").join("main.rs")
    }

    /// Location of the template `Cargo.toml` file that ships with the SQL
    /// compiler.
    pub(crate) fn project_toml_template_path(&self) -> PathBuf {
        Path::new(&self.sql_compiler_home)
            .join("temp")
            .join("Cargo.toml")
    }

    /// Path to the generated `Cargo.toml` file for the project.
    pub(crate) fn project_toml_path(&self, project_id: ProjectId) -> PathBuf {
        self.project_dir(project_id).join("Cargo.toml")
    }

    /// Top-level `Cargo.toml` file for the generated Rust workspace.
    pub(crate) fn workspace_toml_path(&self) -> PathBuf {
        self.workspace_dir().join("Cargo.toml")
    }

    /// Location of the compiled executable for the project.
    pub(crate) fn project_executable(&self, project_id: ProjectId) -> PathBuf {
        Path::new(&self.workspace_dir())
            .join("target")
            .join(if self.debug { "debug" } else { "release" })
            .join(Self::crate_name(project_id))
    }

    /// Location to store pipeline files at runtime.
    pub(crate) fn pipeline_dir(&self, pipeline_id: PipelineId) -> PathBuf {
        Path::new(&self.working_directory)
            .join("pipelines")
            .join(format!("pipeline{pipeline_id}"))
    }

    /// Location to write the pipeline config file.
    pub(crate) fn config_file_path(&self, pipeline_id: PipelineId) -> PathBuf {
        self.pipeline_dir(pipeline_id).join("config.yaml")
    }

    /// Location to write the pipeline metadata file.
    pub(crate) fn metadata_file_path(&self, pipeline_id: PipelineId) -> PathBuf {
        self.pipeline_dir(pipeline_id).join("metadata.json")
    }

    /// Location to redirect the pipeline stderr stream (where the pipeline
    /// writes its log records).
    pub(crate) fn log_file_path(&self, pipeline_id: PipelineId) -> PathBuf {
        self.pipeline_dir(pipeline_id).join("pipeline.log")
    }

    /// Location to redirect the pipeline stdout stream.
    pub(crate) fn out_file_path(&self, pipeline_id: PipelineId) -> PathBuf {
        self.pipeline_dir(pipeline_id).join("pipeline.out")
    }

    /// Directory to store all Prometheus-related files.
    pub(crate) fn prometheus_dir(&self) -> PathBuf {
        Path::new(&self.working_directory).join("prometheus")
    }

    /// Prometheus server config file.
    pub(crate) fn prometheus_server_config_file(&self) -> PathBuf {
        Path::new(&self.working_directory).join("prometheus.yaml")
    }

    /// Prometheus config file for a pipeline.
    pub(crate) fn prometheus_pipeline_config_file(&self, pipeline_id: PipelineId) -> PathBuf {
        self.prometheus_dir()
            .join(format!("pipeline{pipeline_id}.yaml"))
    }
}
