//! Memory management
//!
//! Unified address
//! ---------------
//!
//! - All memories are mapped into a single 64bit memory space
//! - We can get where the pointed memory exists from its value.
//!
//! Memory Types
//! ------------
//!
//! |name                      | where exists | From Host | From Device | As slice | Description                                                            |
//! |:-------------------------|:------------:|:---------:|:-----------:|:--------:|:-----------------------------------------------------------------------|
//! | (usual) Host memory      | Host         | ✓         |  -          |  ✓       | allocated by usual manner, e.g. `vec![0; n]`                           |
//! | [Registered Host memory] | Host         | ✓         |  ✓          |  ✓       | A host memory registered into CUDA memory management system            |
//! | [Page-locked Host memory]| Host         | ✓         |  ✓          |  ✓       | OS memory paging is disabled for accelerating memory transfer          |
//! | [Device memory]          | Device       | ✓         |  ✓          |  ✓       | allocated on device as a single span                                   |
//! | [Array]                  | Device       | ✓         |  ✓          |  -       | properly aligned memory on device for using Texture and Surface memory |
//!
//! [Registered Host memory]:  ./struct.RegisterdMemory.html
//! [Page-locked Host memory]: ./struct.PageLockedMemory.html
//! [Device memory]:           ./struct.DeviceMemory.html
//! [Array]:                   ./struct.Array.html
//!

mod array;
mod device;
mod host;
mod info;
mod scalar;
mod slice;

pub use array::*;
pub use device::*;
pub use host::*;
pub use info::*;
pub use scalar::*;

use crate::*;
use cuda::*;
use std::{mem::MaybeUninit, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum MemoryType {
    Host,
    Registered,
    PageLocked,
    Device,
    Array,
}

/// Typed wrapper of cuPointerGetAttribute
fn get_attr<T, Attr>(ptr: *const T, attr: CUpointer_attribute) -> error::Result<Attr> {
    let data = MaybeUninit::uninit();
    unsafe {
        ffi_call!(
            cuPointerGetAttribute,
            data.as_ptr() as *mut _,
            attr,
            ptr as CUdeviceptr
        )?;
        data.assume_init()
    }
}

/// Has unique head address and allocated size.
pub trait Memory {
    /// Scalar type of each element
    type Elem: Scalar;

    /// Get head address of the memory as a const pointer
    fn head_addr(&self) -> *const Self::Elem;

    /// Get head address of the memory as a mutable pointer
    fn head_addr_mut(&mut self) -> *mut Self::Elem;

    /// Get byte size of allocated memory
    fn byte_size(&self) -> usize;

    /// Get memory type
    fn memory_type(&self) -> MemoryType;

    /// Try to convert into a slice. Return None if the memory is not `Continuous`
    fn try_as_slice(&self) -> Option<&[Self::Elem]>;

    /// Try to convert into a slice. Return error if the memory is not continuous
    fn try_as_mut_slice(&mut self) -> Option<&mut [Self::Elem]>;

    /// Try to get CUDA context. Return None if the memory is not `Contexted`
    fn try_get_context(&self) -> Option<Arc<Context>>;

    /// Copy data from one to another
    ///
    /// Examples
    /// ---------
    ///
    /// - memcpy from page-locked host memory to device memory
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// let src = PageLockedMemory::<i32>::new(ctx.clone(), 12);
    /// dest.copy_from(&src);
    /// ```
    ///
    /// - memcpy from device memory to page-locked host memory
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = PageLockedMemory::<i32>::new(ctx.clone(), 12);
    /// let src = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// dest.copy_from(&src);
    /// ```
    ///
    /// - memcpy from device to device
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// let src = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// dest.copy_from(&src);
    /// ```
    ///
    /// - memcpy from Rust slice to device memory
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// let src = vec![0_i32; 12];
    /// dest.copy_from(src.as_slice()); // requires explicit cast to slice
    /// ```
    ///
    /// - memcpy from device memory to Rust slice
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = vec![0_i32; 12];
    /// let src = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// dest.copy_from(&src);
    /// ```
    ///
    /// Requirements
    /// -------------
    ///
    /// - Cannot copy between different types
    ///
    /// ```compile_fail
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = DeviceMemory::<i64>::new(ctx.clone(), 12);
    /// let src = PageLockedMemory::<i32>::new(ctx.clone(), 12);
    /// dest.copy_from(&src); // compile fail
    /// ```
    ///
    /// - Panics if sizes are different
    ///
    /// ```should_panic
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut dest = DeviceMemory::<i32>::new(ctx.clone(), 24);
    /// let src = PageLockedMemory::<i32>::new(ctx.clone(), 12);
    /// dest.copy_from(&src); // will panic
    /// ```
    ///
    /// Panic
    /// -----
    /// - `self` and `src` are identical
    /// - if `self` nad `src` belong to different context
    /// - if the size memory size mismathes
    fn copy_from<Source>(&mut self, src: &Source)
    where
        Source: Memory<Elem = Self::Elem> + ?Sized;

    /// Set all elements by `value`
    ///
    /// Examples
    /// ---------
    ///
    /// - Set `i32`
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut mem = DeviceMemory::<i32>::new(ctx.clone(), 12);
    /// mem.set(1234);
    /// assert_eq!(mem[10], 1234);
    /// ```
    ///
    /// - Set `f32`
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut mem = DeviceMemory::<f32>::new(ctx.clone(), 12);
    /// mem.set(1.0);
    /// assert_eq!(mem[10], 1.0);
    /// ```
    ///
    /// - Set `f64`. CUDA memset does not support `f64`.
    ///   `set` uses direct access and this will be slow.
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut mem = DeviceMemory::<f64>::new(ctx.clone(), 12);
    /// mem.set(1.0);
    /// assert_eq!(mem[10], 1.0);
    /// ```
    ///
    /// - Set for host memory equals to `mem.iter_mut().for_each(|v| *v = value)`
    ///
    /// ```
    /// # use accel::*;
    /// # let device = Device::nth(0).unwrap();
    /// # let ctx = device.create_context();
    /// let mut mem = PageLockedMemory::<i32>::new(ctx.clone(), 12);
    /// mem.set(1234);
    /// assert_eq!(mem[10], 1234);
    /// ```
    fn set(&mut self, value: Self::Elem);
}

/// Memory which has continuous 1D index, i.e. can be treated as a Rust slice
pub trait Continuous: Memory {
    fn length(&self) -> usize;
    fn as_slice(&self) -> &[Self::Elem];
    fn as_mut_slice(&mut self) -> &mut [Self::Elem];
}

/// Memory which is managed under the CUDA unified memory management systems
pub trait Managed: Memory {
    fn buffer_id(&self) -> u64 {
        get_attr(
            self.head_addr(),
            CUpointer_attribute::CU_POINTER_ATTRIBUTE_BUFFER_ID,
        )
        .expect("Not managed by CUDA")
    }
}