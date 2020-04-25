//! Device and Host memory handlers

use super::*;
use crate::*;
use cuda::*;
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// Memory allocated as page-locked
pub struct PageLockedMemory<T> {
    ptr: *mut T,
    size: usize,
    context: Arc<Context>,
}

impl<T> Drop for PageLockedMemory<T> {
    fn drop(&mut self) {
        if let Err(e) = unsafe { contexted_call!(self, cuMemFreeHost, self.ptr as *mut _) } {
            log::error!("Cannot free page-locked memory: {:?}", e);
        }
    }
}

impl<T> Deref for PageLockedMemory<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr as _, self.size) }
    }
}

impl<T> DerefMut for PageLockedMemory<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

impl<T> Contexted for PageLockedMemory<T> {
    fn get_context(&self) -> Arc<Context> {
        self.context.clone()
    }
}

impl<T: Scalar> Memory for PageLockedMemory<T> {
    type Elem = T;
    fn head_addr(&self) -> *const T {
        self.ptr as _
    }

    fn head_addr_mut(&mut self) -> *mut T {
        self.ptr as _
    }

    fn byte_size(&self) -> usize {
        self.size * std::mem::size_of::<T>()
    }

    fn memory_type(&self) -> MemoryType {
        MemoryType::PageLocked
    }

    fn try_as_slice(&self) -> Option<&[T]> {
        Some(self.as_slice())
    }

    fn try_as_mut_slice(&mut self) -> Option<&mut [T]> {
        Some(self.as_mut_slice())
    }

    fn try_get_context(&self) -> Option<Arc<Context>> {
        Some(self.get_context())
    }

    fn copy_from<Source>(&mut self, src: &Source)
    where
        Source: Memory<Elem = Self::Elem> + ?Sized,
    {
        unsafe { copy_to_host(self, src) }
    }

    fn set(&mut self, value: Self::Elem) {
        self.iter_mut().for_each(|v| *v = value);
    }
}

/// Safety
/// ------
/// - This works only when `dest` is host memory
#[allow(unused_unsafe)]
pub(super) unsafe fn copy_to_host<T: Scalar, Dest, Src>(dest: &mut Dest, src: &Src)
where
    Dest: Memory<Elem = T> + ?Sized,
    Src: Memory<Elem = T> + ?Sized,
{
    assert_ne!(dest.head_addr(), src.head_addr());
    assert_eq!(dest.byte_size(), src.byte_size());

    match src.memory_type() {
        // From host
        MemoryType::Host | MemoryType::Registered | MemoryType::PageLocked => dest
            .try_as_mut_slice()
            .unwrap()
            .copy_from_slice(src.try_as_slice().unwrap()),
        // From device
        MemoryType::Device => {
            let dest_ptr = dest.head_addr_mut();
            let src_ptr = src.head_addr();
            // context guard
            let _g = match (dest.try_get_context(), src.try_get_context()) {
                (Some(d_ctx), Some(s_ctx)) => {
                    assert_eq!(d_ctx, s_ctx);
                    Some(d_ctx.guard_context())
                }
                (Some(ctx), None) => Some(ctx.guard_context()),
                (None, Some(ctx)) => Some(ctx.guard_context()),
                (None, None) => None,
            };
            unsafe {
                ffi_call!(
                    cuMemcpyDtoH_v2,
                    dest_ptr as _,
                    src_ptr as _,
                    dest.byte_size()
                )
            }
            .expect("memcpy from Device to Host failed");
        }
        // From array
        MemoryType::Array => unimplemented!("Array memory is not supported yet"),
    }
}

impl<T: Scalar> Continuous for PageLockedMemory<T> {
    fn length(&self) -> usize {
        self.size
    }
    fn as_slice(&self) -> &[T] {
        self
    }
    fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }
}

impl<T: Scalar> Managed for PageLockedMemory<T> {}

impl<T> PageLockedMemory<T> {
    /// Allocate host memory as page-locked.
    ///
    /// Allocating excessive amounts of pinned memory may degrade system performance,
    /// since it reduces the amount of memory available to the system for paging.
    /// As a result, this function is best used sparingly to allocate staging areas for data exchange between host and device.
    ///
    /// See also [cuMemAllocHost].
    ///
    /// [cuMemAllocHost]: https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__MEM.html#group__CUDA__MEM_1gdd8311286d2c2691605362c689bc64e0
    ///
    /// Panic
    /// ------
    /// - when memory allocation failed includeing `size == 0` case
    ///
    pub fn new(context: Arc<Context>, size: usize) -> Self {
        assert!(size > 0, "Zero-sized malloc is forbidden");
        let ptr =
            unsafe { contexted_new!(&context, cuMemAllocHost_v2, size * std::mem::size_of::<T>()) }
                .expect("Cannot allocate page-locked memory");
        Self {
            ptr: ptr as *mut T,
            size,
            context,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::*;

    #[test]
    fn page_locked() -> Result<()> {
        let device = Device::nth(0)?;
        let ctx = device.create_context();
        let mut mem = PageLockedMemory::<i32>::new(ctx, 12);
        assert_eq!(mem.len(), 12);
        assert_eq!(mem.byte_size(), 12 * 4 /* size of i32 */);
        let sl = mem.as_mut_slice();
        sl[0] = 3;
        Ok(())
    }

    #[should_panic(expected = "Zero-sized malloc is forbidden")]
    #[test]
    fn page_locked_new_zero() {
        let device = Device::nth(0).unwrap();
        let ctx = device.create_context();
        let _a = PageLockedMemory::<i32>::new(ctx, 0);
    }

    #[test]
    fn device() -> Result<()> {
        let device = Device::nth(0)?;
        let ctx = device.create_context();
        let mut mem = DeviceMemory::<i32>::new(ctx, 12);
        assert_eq!(mem.len(), 12);
        assert_eq!(mem.byte_size(), 12 * 4 /* size of i32 */);
        let sl = mem.as_mut_slice();
        sl[0] = 3;
        Ok(())
    }

    #[should_panic(expected = "Zero-sized malloc is forbidden")]
    #[test]
    fn device_new_zero() {
        let device = Device::nth(0).unwrap();
        let ctx = device.create_context();
        let _a = DeviceMemory::<i32>::new(ctx, 0);
    }
}