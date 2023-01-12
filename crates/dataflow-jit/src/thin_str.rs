use std::{
    alloc::Layout,
    cmp::Ordering,
    fmt::{self, Debug, Display},
    marker::PhantomData,
    mem::{align_of, ManuallyDrop},
    ptr::{self, addr_of, addr_of_mut, NonNull},
    slice, str,
};

static EMPTY: StrHeader = StrHeader {
    length: 0,
    capacity: 0,
    _data: [],
};

#[repr(C)]
struct StrHeader {
    length: usize,
    capacity: usize,
    _data: [u8; 0],
}

#[repr(transparent)]
pub struct ThinStrRef<'a> {
    buf: NonNull<StrHeader>,
    __lifetime: PhantomData<&'a ()>,
}

#[repr(transparent)]
pub struct ThinStr {
    buf: NonNull<StrHeader>,
}

impl ThinStr {
    #[inline]
    pub fn new() -> Self {
        Self {
            buf: NonNull::from(&EMPTY),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        unsafe { (*self.buf.as_ptr()).length }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        unsafe { (*self.buf.as_ptr()).capacity }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        unsafe { addr_of!((*self.buf.as_ptr())._data).cast() }
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        unsafe { addr_of_mut!((*self.buf.as_ptr())._data).cast() }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        // Safety: All bytes up to self.len() are valid
        unsafe { slice::from_raw_parts(self.as_ptr(), self.len()) }
    }

    pub unsafe fn set_len(&mut self, length: usize) {
        debug_assert!(self.buf != NonNull::from(&EMPTY));
        unsafe { addr_of_mut!((*self.buf.as_ptr()).length).write(length) }
    }

    #[inline]
    pub fn into_raw(self) -> *mut () {
        let this = ManuallyDrop::new(self);
        this.buf.as_ptr().cast()
    }

    #[inline]
    pub unsafe fn from_raw(raw: *mut ()) -> Self {
        debug_assert!(
            !raw.is_null(),
            "Cannot call `ThinStr::from_raw()` on a null pointer",
        );

        Self {
            buf: NonNull::new_unchecked(raw.cast()),
        }
    }

    unsafe fn with_capacity_uninit(capacity: usize) -> Self {
        let layout = Self::layout_for(capacity);

        let ptr = unsafe { std::alloc::alloc(layout) };
        let buf = match NonNull::new(ptr.cast::<StrHeader>()) {
            Some(buf) => buf,
            None => std::alloc::handle_alloc_error(layout),
        };

        // Write the length (zero) and capacity to the allocation
        unsafe {
            let ptr = buf.as_ptr();
            addr_of_mut!((*ptr).length).write(0);
            addr_of_mut!((*ptr).capacity).write(capacity);
        }

        Self { buf }
    }

    fn layout_for(capacity: usize) -> Layout {
        let header = Layout::new::<StrHeader>();

        let align = align_of::<usize>();
        let bytes = Layout::from_size_align(round_to_align(capacity, align), align)
            .expect("failed to create layout for string bytes");

        let (layout, _) = header
            .extend(bytes)
            .expect("failed to add header and string bytes layouts");

        // Pad out the layout
        layout.pad_to_align()
    }
}

impl Default for ThinStr {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ThinStr {
    fn clone(&self) -> Self {
        if self.is_empty() {
            return Self::new();
        }

        unsafe {
            let length = self.len();

            let mut new = Self::with_capacity_uninit(length);
            ptr::copy_nonoverlapping(self.as_ptr(), new.as_mut_ptr(), length);
            new.set_len(length);

            new
        }
    }

    fn clone_from(&mut self, source: &Self) {
        if source.is_empty() {
            *self = Self::new();
            return;
        }

        if self.capacity() >= source.len() {
            unsafe {
                let length = source.len();
                ptr::copy_nonoverlapping(source.as_ptr(), self.as_mut_ptr(), length);
                self.set_len(length);
            }
        } else {
            *self = source.clone();
        }
    }
}

impl Debug for ThinStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}

impl Display for ThinStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq for ThinStr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str().eq(other.as_str())
    }
}

impl Eq for ThinStr {}

impl PartialOrd for ThinStr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_str().partial_cmp(other.as_str())
    }
}

impl Ord for ThinStr {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

const fn round_to_align(size: usize, align: usize) -> usize {
    size.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1)
}