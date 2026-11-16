//! Owned, checked wrappers for the stable GDAL C APIs used by the converter.
//! Bindings are generated against installed headers; desktop rendering uses no FFI.
use gdal_sys as sys;
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    path::Path,
    ptr,
    sync::Once,
};
type Result<T> = std::result::Result<T, String>;
fn c(s: &str) -> Result<CString> {
    CString::new(s).map_err(|e| e.to_string())
}
fn failure() -> String {
    // GDAL owns a thread-local, nul-terminated error string.
    unsafe {
        CStr::from_ptr(sys::CPLGetLastErrorMsg())
            .to_string_lossy()
            .into_owned()
    }
}
fn check(code: sys::CPLErr::Type) -> Result<()> {
    if code == sys::CPLErr::CE_None {
        Ok(())
    } else {
        Err(failure())
    }
}
fn initialize() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe { sys::GDALAllRegister() });
}

pub struct SpatialRef(sys::OGRSpatialReferenceH);
impl Drop for SpatialRef {
    fn drop(&mut self) {
        unsafe {
            sys::OSRRelease(self.0);
        }
    }
}
impl SpatialRef {
    pub fn from_definition(s: &str) -> Result<Self> {
        let s = c(s)?;
        // The returned reference is uniquely owned here and released by Drop.
        let handle = unsafe { sys::OSRNewSpatialReference(ptr::null()) };
        if handle.is_null() {
            return Err(failure());
        }
        let result = Self(handle);
        if unsafe { sys::OSRSetFromUserInput(handle, s.as_ptr()) } != 0 {
            return Err(failure());
        }
        unsafe {
            sys::OSRSetAxisMappingStrategy(
                handle,
                sys::OSRAxisMappingStrategy::OAMS_TRADITIONAL_GIS_ORDER,
            )
        };
        Ok(result)
    }
    pub fn from_wkt(s: &str) -> Result<Self> {
        Self::from_definition(s)
    }
    pub fn from_epsg(epsg: u32) -> Result<Self> {
        Self::from_definition(&format!("EPSG:{epsg}"))
    }
    pub fn to_wkt(&self) -> Result<String> {
        let mut out = ptr::null_mut();
        if unsafe { sys::OSRExportToWkt(self.0, &mut out) } != 0 || out.is_null() {
            return Err(failure());
        }
        // Export allocates with GDAL's allocator; copy before freeing with VSIFree.
        let text = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        unsafe { sys::VSIFree(out.cast()) };
        Ok(text)
    }
    pub fn horizontal(&self) -> Result<Self> {
        let handle = unsafe { sys::OSRClone(self.0) };
        if handle.is_null() {
            return Err(failure());
        }
        let result = Self(handle);
        // Demotion removes vertical axes without transforming observed Z values.
        if unsafe { sys::OSRDemoteTo2D(handle, ptr::null()) } != 0 {
            return Err(failure());
        }
        if unsafe { sys::OSRGetAxesCount(handle) } != 2
            || unsafe { sys::OSRIsGeographic(handle) } == 0
                && unsafe { sys::OSRIsProjected(handle) } == 0
        {
            return Err("Unsupported horizontal CRS".into());
        }
        unsafe {
            sys::OSRSetAxisMappingStrategy(
                handle,
                sys::OSRAxisMappingStrategy::OAMS_TRADITIONAL_GIS_ORDER,
            )
        };
        Ok(result)
    }
}

pub struct CoordTransform {
    handle: sys::OGRCoordinateTransformationH,
    _source: SpatialRef,
    _target: SpatialRef,
}
impl Drop for CoordTransform {
    fn drop(&mut self) {
        unsafe { sys::OCTDestroyCoordinateTransformation(self.handle) }
    }
}
impl CoordTransform {
    pub fn new(source: SpatialRef, target: SpatialRef) -> Result<Self> {
        let handle = unsafe { sys::OCTNewCoordinateTransformation(source.0, target.0) };
        if handle.is_null() {
            return Err(failure());
        }
        Ok(Self {
            handle,
            _source: source,
            _target: target,
        })
    }
    pub fn transform_bounds(&self, bounds: &[f64; 4], densify: i32) -> Result<[f64; 4]> {
        let mut out = [0.; 4];
        let ok = unsafe {
            sys::OCTTransformBounds(
                self.handle,
                bounds[0],
                bounds[1],
                bounds[2],
                bounds[3],
                &mut out[0],
                &mut out[1],
                &mut out[2],
                &mut out[3],
                densify,
            )
        };
        if ok == 0 { Err(failure()) } else { Ok(out) }
    }
    pub fn transform_coords(&self, x: &mut [f64], y: &mut [f64]) -> Result<()> {
        if x.len() != y.len() || x.len() > i32::MAX as usize {
            return Err("Coordinate buffer length mismatch".into());
        }
        if x.is_empty() {
            return Ok(());
        }
        // Both buffers contain n writable doubles; null Z intentionally leaves
        // the source's native vertical measurements outside this XY operation.
        let ok = unsafe {
            sys::OCTTransform(
                self.handle,
                x.len() as i32,
                x.as_mut_ptr(),
                y.as_mut_ptr(),
                ptr::null_mut(),
            )
        };
        if ok == 0 { Err(failure()) } else { Ok(()) }
    }
}

pub struct Dataset(sys::GDALDatasetH);
impl Drop for Dataset {
    fn drop(&mut self) {
        unsafe {
            sys::GDALClose(self.0);
        }
    }
}
impl Dataset {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_mode(path, false)
    }
    pub fn update(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_mode(path, true)
    }
    fn open_mode(path: impl AsRef<Path>, update: bool) -> Result<Self> {
        initialize();
        let path = c(&path.as_ref().to_string_lossy())?;
        let handle = unsafe {
            sys::GDALOpen(
                path.as_ptr(),
                if update {
                    sys::GDALAccess::GA_Update
                } else {
                    sys::GDALAccess::GA_ReadOnly
                },
            )
        };
        if handle.is_null() {
            Err(failure())
        } else {
            Ok(Self(handle))
        }
    }
    pub fn create(path: &Path, width: usize, height: usize) -> Result<Self> {
        Self::create_layout(path, width, height, true)
    }
    pub(crate) fn create_layout(
        path: &Path,
        width: usize,
        height: usize,
        planar: bool,
    ) -> Result<Self> {
        initialize();
        let path = c(&path.to_string_lossy())?;
        let gtiff = c("GTiff")?;
        if width == 0 || height == 0 || width > i32::MAX as usize || height > i32::MAX as usize {
            return Err("Invalid raster dimensions".into());
        }
        let options = [
            "COMPRESS=DEFLATE",
            "PREDICTOR=3",
            "TILED=YES",
            if planar {
                "INTERLEAVE=BAND"
            } else {
                "INTERLEAVE=PIXEL"
            },
        ]
        .into_iter()
        .map(c)
        .collect::<Result<Vec<_>>>()?;
        let mut pointers: Vec<_> = options.iter().map(|s| s.as_ptr() as *mut _).collect();
        pointers.push(ptr::null_mut());
        let driver = unsafe { sys::GDALGetDriverByName(gtiff.as_ptr()) };
        if driver.is_null() {
            return Err("GDAL GTiff driver unavailable".into());
        }
        let handle = unsafe {
            sys::GDALCreate(
                driver,
                path.as_ptr(),
                width as i32,
                height as i32,
                4,
                sys::GDALDataType::GDT_Float32,
                pointers.as_mut_ptr(),
            )
        };
        if handle.is_null() {
            Err(failure())
        } else {
            Ok(Self(handle))
        }
    }
    pub fn set_spatial_ref(&mut self, srs: &SpatialRef) -> Result<()> {
        check(unsafe { sys::GDALSetSpatialRef(self.0, srs.0) })
    }
    pub fn set_geo_transform(&mut self, transform: &[f64; 6]) -> Result<()> {
        let mut t = *transform;
        check(unsafe { sys::GDALSetGeoTransform(self.0, t.as_mut_ptr()) })
    }
    pub fn set_metadata_item(&mut self, key: &str, value: &str, domain: &str) -> Result<()> {
        let (key, value, domain) = (c(key)?, c(value)?, c(domain)?);
        check(unsafe {
            sys::GDALSetMetadataItem(self.0, key.as_ptr(), value.as_ptr(), domain.as_ptr())
        })
    }
    pub fn raster_size(&self) -> (usize, usize) {
        unsafe {
            (
                sys::GDALGetRasterXSize(self.0) as usize,
                sys::GDALGetRasterYSize(self.0) as usize,
            )
        }
    }
    pub fn raster_count(&self) -> usize {
        unsafe { sys::GDALGetRasterCount(self.0) as usize }
    }
    pub fn rasterband(&self, band: usize) -> Result<Band<'_>> {
        if band == 0 || band > self.raster_count() {
            return Err("Invalid raster band".into());
        }
        let handle = unsafe { sys::GDALGetRasterBand(self.0, band as i32) };
        if handle.is_null() {
            return Err(failure());
        }
        Ok(Band {
            handle,
            size: self.raster_size(),
            _dataset: PhantomData,
        })
    }
    pub fn flush_cache(&mut self) -> Result<()> {
        check(unsafe { sys::GDALFlushCache(self.0) })
    }
}
pub struct Band<'a> {
    handle: sys::GDALRasterBandH,
    size: (usize, usize),
    _dataset: PhantomData<&'a Dataset>,
}
impl Band<'_> {
    pub fn set_no_data_value(&mut self, value: f64) -> Result<()> {
        check(unsafe { sys::GDALSetRasterNoDataValue(self.handle, value) })
    }
    pub fn set_description(&mut self, value: &str) -> Result<()> {
        let value = c(value)?;
        unsafe { sys::GDALSetDescription(self.handle, value.as_ptr()) };
        Ok(())
    }
    pub fn description(&self) -> Result<String> {
        let ptr = unsafe { sys::GDALGetDescription(self.handle) };
        if ptr.is_null() {
            Err(failure())
        } else {
            Ok(unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned())
        }
    }
    fn raster_io(&self, data: &mut [f32], write: bool) -> Result<()> {
        let (w, h) = self.size;
        if w.checked_mul(h) != Some(data.len()) {
            return Err("Raster buffer length mismatch".into());
        }
        // GDALRasterIO is the stable API without versioned ExtraArg structures.
        // The buffer length was checked for the entire requested float32 band.
        check(unsafe {
            sys::GDALRasterIO(
                self.handle,
                if write {
                    sys::GDALRWFlag::GF_Write
                } else {
                    sys::GDALRWFlag::GF_Read
                },
                0,
                0,
                w as i32,
                h as i32,
                data.as_mut_ptr().cast(),
                w as i32,
                h as i32,
                sys::GDALDataType::GDT_Float32,
                0,
                0,
            )
        })
    }
    pub fn write(&mut self, values: &mut [f32]) -> Result<()> {
        self.raster_io(values, true)
    }
    pub fn read(&self) -> Result<Vec<f32>> {
        let n = self
            .size
            .0
            .checked_mul(self.size.1)
            .filter(|n| *n <= 16_000_000)
            .ok_or("Oversized numeric raster")?;
        let mut result = vec![0.; n];
        self.raster_io(&mut result, false)?;
        Ok(result)
    }
}
