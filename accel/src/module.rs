//! Resource of CUDA middle-IR (PTX/cubin)
//!
//! This module includes a wrapper of `cuLink*` and `cuModule*`
//! in [CUDA Driver APIs](http://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__MODULE.html).

use super::{device::*, instruction::*, *};
use crate::{error::Result, ffi_call, ffi_new};
use cuda::*;
use std::{ffi::*, path::Path, ptr::null_mut};

/// CUDA Kernel function
#[derive(Debug)]
pub struct Kernel<'ctx> {
    func: CUfunction,
    module: &'ctx Module<'ctx>,
}

impl<'ctx> Contexted for Kernel<'ctx> {
    fn get_context(&self) -> &Context {
        self.module.get_context()
    }
}

/// Type which can be sent to the device as kernel argument
///
/// ```
/// # use accel::module::*;
/// # use std::ffi::*;
/// let a: i32 = 10;
/// let p = &a as *const i32;
/// assert_eq!(
///     DeviceSend::as_ptr(&p),
///     &p as *const *const i32 as *const u8
/// );
/// assert!(std::ptr::eq(
///     unsafe { *(DeviceSend::as_ptr(&p) as *mut *const i32) },
///     p
/// ));
/// ```
pub trait DeviceSend: Sized {
    /// Get the address of this value
    fn as_ptr(&self) -> *const u8 {
        self as *const Self as *const u8
    }
}

// Use default impl
impl<T> DeviceSend for *mut T {}
impl<T> DeviceSend for *const T {}
impl DeviceSend for bool {}
impl DeviceSend for i8 {}
impl DeviceSend for i16 {}
impl DeviceSend for i32 {}
impl DeviceSend for i64 {}
impl DeviceSend for isize {}
impl DeviceSend for u8 {}
impl DeviceSend for u16 {}
impl DeviceSend for u32 {}
impl DeviceSend for u64 {}
impl DeviceSend for usize {}
impl DeviceSend for f32 {}
impl DeviceSend for f64 {}

/// Arbitary number of tuple of kernel arguments
///
/// ```
/// # use accel::module::*;
/// # use std::ffi::*;
/// let a: i32 = 10;
/// let b: f32 = 1.0;
/// assert_eq!(
///   Arguments::kernel_params(&(&a, &b)),
///   vec![&a as *const i32 as *mut _, &b as *const f32 as *mut _, ]
/// );
/// ```
pub trait Arguments<'arg> {
    /// Get a list of kernel parameters to be passed into [cuLaunchKernel]
    ///
    /// [cuLaunchKernel]: https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__EXEC.html#group__CUDA__EXEC_1gb8f3dc3031b40da29d5f9a7139e52e15
    fn kernel_params(&self) -> Vec<*mut c_void>;
}

macro_rules! impl_kernel_parameters {
    ($($name:ident),*; $($num:tt),*) => {
        impl<'arg, $($name : DeviceSend),*> Arguments<'arg> for ($( &'arg $name, )*) {
            fn kernel_params(&self) -> Vec<*mut c_void> {
                vec![$( self.$num.as_ptr() as *mut c_void ),*]
            }
        }
    }
}

impl_kernel_parameters!(;);
impl_kernel_parameters!(D0; 0);
impl_kernel_parameters!(D0, D1; 0, 1);
impl_kernel_parameters!(D0, D1, D2; 0, 1, 2);
impl_kernel_parameters!(D0, D1, D2, D3; 0, 1, 2, 3);
impl_kernel_parameters!(D0, D1, D2, D3, D4; 0, 1, 2, 3, 4);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5; 0, 1, 2, 3, 4, 5);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5, D6; 0, 1, 2, 3, 4, 5, 6);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5, D6, D7; 0, 1, 2, 3, 4, 5, 6, 7);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5, D6, D7, D8; 0, 1, 2, 3, 4, 5, 6, 7, 8);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5, D6, D7, D8, D9; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5, D6, D7, D8, D9, D10; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10);
impl_kernel_parameters!(D0, D1, D2, D3, D4, D5, D6, D7, D8, D9, D10, D11; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);

/// Typed CUDA Kernel launcher
///
/// This will be automatically implemented in [accel_derive::kernel] for autogenerated wrapper
/// module of [Module].
///
/// ```
/// #[accel_derive::kernel]
/// fn f(a: i32) {}
/// ```
///
/// will create a submodule `f`:
///
/// ```
/// mod f {
///     pub const PTX_STR: &'static str = "PTX string generated by rustc/nvptx64-nvidia-cuda";
///     pub struct Module<'ctx>(::accel::module::Module<'ctx>);
///     /* impl Module { ... } */
///     /* impl Launchable for Module { ... } */
/// }
/// ```
///
/// Implementation of `Launchable` for `f::Module` is also generated by [accel_derive::kernel]
/// proc-macro.
///
/// [accel_derive::kernel]: https://docs.rs/accel-derive/0.3.0-alpha.1/accel_derive/attr.kernel.html
/// [Module]: struct.Module.html
pub trait Launchable<'arg> {
    /// Arguments for the kernel to be launched.
    /// This must be a tuple of [DeviceSend] types.
    ///
    /// [DeviceSend]: trait.DeviceSend.html
    type Args: Arguments<'arg>;

    fn get_kernel(&self) -> Result<Kernel>;

    /// Launch CUDA Kernel
    ///
    /// ```
    /// use accel::{module::Launchable, device::*, memory::*, *};
    ///
    /// #[accel_derive::kernel]
    /// fn f(a: i32) {}
    ///
    /// let device = Device::nth(0)?;
    /// let ctx = device.create_context();
    /// let grid = Grid::x(1);
    /// let block = Block::x(4);
    ///
    /// let module = f::Module::new(&ctx)?;
    /// let a = 12;
    /// module.launch(grid, block, &(&a,))?;
    /// # Ok::<(), ::accel::error::AccelError>(())
    /// ```
    fn launch(&self, grid: Grid, block: Block, args: &Self::Args) -> Result<()> {
        let kernel = self.get_kernel()?;
        let _g = kernel.guard_context();
        let mut params = args.kernel_params();
        Ok(ffi_call!(
            cuLaunchKernel,
            kernel.func,
            grid.x,
            grid.y,
            grid.z,
            block.x,
            block.y,
            block.z,
            0,          /* FIXME: no shared memory */
            null_mut(), /* use default stream */
            params.as_mut_ptr(),
            null_mut() /* no extra */
        )?)
    }
}

/// OOP-like wrapper of `cuModule*` APIs
#[derive(Debug)]
pub struct Module<'ctx> {
    module: CUmodule,
    context: &'ctx Context,
}

impl<'ctx> Drop for Module<'ctx> {
    fn drop(&mut self) {
        ffi_call!(cuModuleUnload, self.module).expect("Failed to unload module");
    }
}

impl<'ctx> Contexted for Module<'ctx> {
    fn get_context(&self) -> &Context {
        self.context
    }
}

impl<'ctx> Module<'ctx> {
    /// integrated loader of Instruction
    pub fn load(context: &'ctx Context, data: &Instruction) -> Result<Self> {
        let _gurad = context.guard_context();
        match *data {
            Instruction::PTX(ref ptx) => {
                let module = ffi_new!(cuModuleLoadData, ptx.as_ptr() as *const _)?;
                Ok(Module { module, context })
            }
            Instruction::Cubin(ref bin) => {
                let module = ffi_new!(cuModuleLoadData, bin.as_ptr() as *const _)?;
                Ok(Module { module, context })
            }
            Instruction::PTXFile(ref path) | Instruction::CubinFile(ref path) => {
                let filename = path_to_cstring(path);
                let module = ffi_new!(cuModuleLoad, filename.as_ptr())?;
                Ok(Module { module, context })
            }
        }
    }

    pub fn from_str(context: &'ctx Context, ptx: &str) -> Result<Self> {
        let data = Instruction::ptx(ptx);
        Self::load(context, &data)
    }

    /// Wrapper of `cuModuleGetFunction`
    pub fn get_kernel(&self, name: &str) -> Result<Kernel> {
        let _gurad = self.guard_context();
        let name = CString::new(name).expect("Invalid Kernel name");
        let func = ffi_new!(cuModuleGetFunction, self.module, name.as_ptr())?;
        Ok(Kernel { func, module: self })
    }
}

fn path_to_cstring(path: &Path) -> CString {
    CString::new(path.to_str().unwrap()).expect("Invalid Path")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_do_nothing() -> Result<()> {
        // generated by do_nothing example in accel-derive
        let ptx = r#"
        .version 3.2
        .target sm_30
        .address_size 64
        .visible .entry do_nothing()
        {
          ret;
        }
        "#;
        let device = Device::nth(0)?;
        let ctx = device.create_context();
        let _mod = Module::from_str(&ctx, ptx)?;
        Ok(())
    }
}
