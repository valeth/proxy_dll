use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Ident};

#[proc_macro]
pub fn proxy_dll(input: TokenStream) -> TokenStream {
    let main = parse_macro_input!(input as Ident);

    let target = "winmm";

    let dll = format!("{target}.dll");

    let file_string = include_str!("../../defs/winmm.def");
    // preprocess out comments because parsing lib can't handle them?
    let file_string = file_string
        .lines()
        .filter(|l| !l.starts_with(";"))
        .collect::<Vec<&str>>()
        .join("\n");
    let file = msvc_def::parse_ref(&file_string).unwrap();

    let mut export_fns = vec![];
    let mut setup_forwards = vec![];

    let mut count: usize = 0;
    for (i, e) in file.exports.enumerate() {
        count += 1;
        let export = e.unwrap();

        let name = export.name;
        if name == "DllMain" {
            continue;
        }

        let name_ident = format_ident!("{name}");

        export_fns.push(quote! {
            #[no_mangle]
            pub unsafe extern "C" fn #name_ident() {
                core::mem::transmute::<usize, unsafe extern "C" fn()>(FORWARDS[#i])();
            }
        });

        let cstr_name = std::ffi::CString::new(name).unwrap();

        let name_or_ord = if export.noname {
            let ord = export.ordinal.unwrap();
            quote! {
                #ord as *const u8
            }
        } else {
            quote! {
                #cstr_name.as_ptr() as *const u8
            }
        };

        setup_forwards.push(quote! {
            FORWARDS[#i] = GetProcAddress(TARGET_DLL_HANDLE, #name_or_ord).unwrap_unchecked() as usize;
        });
    }

    let output = quote! {
        unsafe extern "system" fn __call_main(_: usize) {
            #main();
        }
        #[cfg(windows)]
        mod __proxy_dll {
            const TARGET: &str = #dll;
            static mut FORWARDS: [usize; #count] = [0; #count];

            mod exports {
                use super::*;
                #(#export_fns)*
            }

            unsafe fn setup_forwards() {
                #(#setup_forwards)*
            }

            use ::proxy_dll::windows_sys::{
                Win32::{
                    Foundation::{FreeLibrary, BOOL, HMODULE, MAX_PATH},
                    System::{
                        LibraryLoader::{GetProcAddress, LoadLibraryA, LoadLibraryW},
                        SystemInformation::GetSystemDirectoryW,
                        SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH},
                        Threading::{ExitProcess, GetCurrentProcess, GetCurrentThread, QueueUserAPC},
                    },
                },
            };

            static mut TARGET_DLL_HANDLE: HMODULE = std::ptr::null_mut();
            static mut INIT: bool = false;

            unsafe fn load_target() {
                let mut path = [0u16; MAX_PATH as usize];

                let len = GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) as usize;
                if len == 0 {
                    return;
                }

                use std::os::windows::ffi::{OsStringExt, OsStrExt};
                let os_str = std::ffi::OsString::from_wide(&path[..len]);
                let p = std::path::Path::new(&os_str);

                let mut dll_path = p.join(TARGET).into_os_string();
                dll_path.push("\0");

                TARGET_DLL_HANDLE = LoadLibraryW(dll_path.encode_wide().collect::<Vec<u16>>().as_ptr());

                if TARGET_DLL_HANDLE == std::ptr::null_mut() {
                    ExitProcess(0);
                }
            }

            #[no_mangle]
            pub extern "system" fn DllMain(
                _hinst_dll: HMODULE,
                fdw_reason: u32,
                _lpv_reserved: *mut core::ffi::c_void,
            ) -> BOOL {
                match fdw_reason {
                    DLL_PROCESS_ATTACH => unsafe {
                        if !INIT {
                            INIT = true;
                            load_target();
                            setup_forwards();
                            QueueUserAPC(Some(super::__call_main), GetCurrentThread(), 0);
                        }
                    },
                    DLL_PROCESS_DETACH => unsafe {
                        FreeLibrary(TARGET_DLL_HANDLE);
                    },
                    _ => {}
                }
                true.into()
            }
        }
    };

    output.into()
}
