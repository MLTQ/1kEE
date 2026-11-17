//! Optional runtime PROJ binding. Each point-loading thread owns a private,
//! offline context and reuses the normalized horizontal transform across batches.
use libloading::Library;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    ptr,
    sync::OnceLock,
};

type Handle = *mut c_void;
type Destroy = unsafe extern "C" fn(Handle) -> Handle;
type Transform = unsafe extern "C" fn(
    Handle,
    c_int,
    *mut f64,
    usize,
    usize,
    *mut f64,
    usize,
    usize,
    *mut f64,
    usize,
    usize,
    *mut f64,
    usize,
    usize,
) -> usize;
struct Api {
    context: unsafe extern "C" fn() -> Handle,
    end_context: Destroy,
    network: unsafe extern "C" fn(Handle, c_int) -> c_int,
    create: unsafe extern "C" fn(Handle, *const c_char, *const c_char, Handle) -> Handle,
    normalize: unsafe extern "C" fn(Handle, Handle) -> Handle,
    destroy: Destroy,
    transform: Transform,
    _library: Library,
}
impl Api {
    fn load() -> Option<Self> {
        // Search installed system libraries only. The CLI remains available on
        // installations using a separately configured GDAL distribution.
        let names: &[&str] = if cfg!(target_os = "linux") {
            &["libproj.so.25", "libproj.so"]
        } else if cfg!(target_os = "macos") {
            &[
                "/opt/homebrew/lib/libproj.dylib",
                "/usr/local/lib/libproj.dylib",
            ]
        } else {
            &[]
        };
        // Types below match the public C signatures in proj.h. Keep the library
        // loaded for the lifetime of every copied function pointer.
        unsafe {
            let lib = names.iter().find_map(|name| Library::new(name).ok())?;
            Some(Self {
                context: *lib.get(b"proj_context_create").ok()?,
                end_context: *lib.get(b"proj_context_destroy").ok()?,
                network: *lib.get(b"proj_context_set_enable_network").ok()?,
                create: *lib.get(b"proj_create_crs_to_crs").ok()?,
                normalize: *lib.get(b"proj_normalize_for_visualization").ok()?,
                destroy: *lib.get(b"proj_destroy").ok()?,
                transform: *lib.get(b"proj_trans_generic").ok()?,
                _library: lib,
            })
        }
    }
}
pub(super) struct Projection {
    api: &'static Api,
    context: Handle,
    operation: Handle,
    pub crs: String,
}
impl Projection {
    pub fn new(crs: &str) -> Option<Self> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        let api = API.get_or_init(Api::load).as_ref()?;
        let source = CString::new(crs).ok()?;
        // Successful handles become owned by this worker. Normalization
        // establishes traditional lon/lat and projected XY axis order.
        unsafe {
            let context = (api.context)();
            if context.is_null() {
                return None;
            }
            (api.network)(context, 0);
            let raw = (api.create)(
                context,
                source.as_ptr(),
                c"EPSG:4326".as_ptr(),
                ptr::null_mut(),
            );
            let operation = if raw.is_null() {
                raw
            } else {
                let xy = (api.normalize)(context, raw);
                (api.destroy)(raw);
                xy
            };
            if operation.is_null() {
                (api.end_context)(context);
                return None;
            }
            Some(Self {
                api,
                context,
                operation,
                crs: crs.into(),
            })
        }
    }
    pub fn convert(&self, points: &mut [[f64; 2]]) -> Result<(), String> {
        let stride = size_of::<[f64; 2]>();
        let n = points.len();
        if n == 0 {
            return Ok(());
        }
        let xy = points.as_mut_ptr().cast::<f64>();
        // The interleaved arrays describe distinct writable X/Y coordinates.
        // Z and time have zero length. PROJ never sees the survey elevations.
        let count = unsafe {
            (self.api.transform)(
                self.operation,
                1,
                xy,
                stride,
                n,
                xy.add(1),
                stride,
                n,
                ptr::null_mut(),
                0,
                0,
                ptr::null_mut(),
                0,
                0,
            )
        };
        if count != n || !points.iter().all(|p| p.iter().all(|v| v.is_finite())) {
            return Err("Invalid projected LAS coordinates".into());
        }
        Ok(())
    }
}
impl Drop for Projection {
    fn drop(&mut self) {
        unsafe {
            (self.api.destroy)(self.operation);
            (self.api.end_context)(self.context);
        }
    }
}
