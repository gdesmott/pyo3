// Copyright (c) 2017-present PyO3 Project and Contributors

use syn;
use quote::Tokens;

#[derive(Debug)]
struct Arg<'a> {
    pub name: &'a syn::Ident,
    pub mode: &'a syn::BindingMode,
    pub ty: &'a syn::Ty,
    pub optional: Option<&'a syn::Ty>,
}


pub fn gen_py_method<'a>(cls: &Box<syn::Ty>, name: &syn::Ident,
                         sig: &mut syn::MethodSig, _block: &mut syn::Block) -> Tokens {
    check_generic(name, sig);

    //let mut has_self = false;
    let mut py = false;
    let mut arguments: Vec<Arg> = Vec::new();

    for input in sig.decl.inputs.iter() {
        match input {
            &syn::FnArg::SelfRef(_, _) => {
                //has_self = true;
            },
            &syn::FnArg::SelfValue(_) => {
                //has_self = true;
            }
            &syn::FnArg::Captured(ref pat, ref ty) => {
                let (mode, ident) = match pat {
                    &syn::Pat::Ident(ref mode, ref ident, _) =>
                        (mode, ident),
                    _ =>
                        panic!("unsupported argument: {:?}", pat),
                };
                // TODO add check for first py: Python arg
                if py {
                    let opt = check_arg_ty_and_optional(name, ty);
                    arguments.push(Arg{name: ident, mode: mode, ty: ty, optional: opt});
                } else {
                    py = true;
                }
            }
            &syn::FnArg::Ignored(_) =>
                panic!("ignored argument: {:?}", name),
        }
    }

    impl_py_method_def(name, &impl_wrap(cls, name, arguments))
}

 fn check_generic(name: &syn::Ident, sig: &syn::MethodSig) {
    if !sig.generics.ty_params.is_empty() {
        panic!("python method can not be generic: {:?}", name);
    }
}

fn check_arg_ty_and_optional<'a>(name: &'a syn::Ident, ty: &'a syn::Ty) -> Option<&'a syn::Ty> {
    match ty {
        &syn::Ty::Path(ref qs, ref path) => {
            if let &Some(ref qs) = qs {
                panic!("explicit Self type in a 'qualified path' is not supported: {:?} - {:?}",
                       name, qs);
            }

            if let Some(segment) = path.segments.last() {
                match segment.ident.as_ref() {
                    "Option" => {
                        match segment.parameters {
                            syn::PathParameters::AngleBracketed(ref params) => {
                                if params.types.len() != 1 {
                                    panic!("argument type is not supported by python method: {:?} ({:?})",
                                           name, ty);
                                }
                                Some(&params.types[0])
                            },
                            _ => {
                                panic!("argument type is not supported by python method: {:?} ({:?})",
                                       name, ty);
                            }
                        }
                    },
                    _ => None,
                }
            } else {
                None
            }
        },
        _ => {
            panic!("argument type is not supported by python method: {:?} ({:?})",
                   name, ty);
        },
    }
}

/// Generate functiona wrapper (PyCFunction, PyCFunctionWithKeywords)
fn impl_wrap(cls: &Box<syn::Ty>, name: &syn::Ident, args: Vec<Arg>) -> Tokens {
    let cb = impl_call(cls, name, &args);
    let body = impl_arg_params(args, cb);

    quote! {
        unsafe extern "C" fn wrap
            (slf: *mut pyo3::ffi::PyObject,
             args: *mut pyo3::ffi::PyObject,
             kwargs: *mut pyo3::ffi::PyObject) -> *mut pyo3::ffi::PyObject
        {
            const LOCATION: &'static str = concat!(
                stringify!(#cls), ".", stringify!(#name), "()");
            pyo3::_detail::handle_callback(
                LOCATION, pyo3::_detail::PyObjectCallbackConverter, |py|
                {
                    let args: pyo3::PyTuple =
                        pyo3::PyObject::from_borrowed_ptr(py, args).unchecked_cast_into();
                    let kwargs: Option<pyo3::PyDict> = pyo3::argparse::get_kwargs(py, kwargs);

                    let ret = {
                        #body
                    };
                    pyo3::PyDrop::release_ref(args, py);
                    pyo3::PyDrop::release_ref(kwargs, py);
                    ret
                })
        }
    }
}

fn impl_call(cls: &Box<syn::Ty>, fname: &syn::Ident, args: &Vec<Arg>) -> Tokens {
    let names: Vec<&syn::Ident> = args.iter().map(|item| item.name).collect();
    quote! {
        {
            let slf = pyo3::PyObject::from_borrowed_ptr(py, slf).unchecked_cast_into::<#cls>();
            let ret = slf.#fname(py, #(#names),*);
            pyo3::PyDrop::release_ref(slf, py);
            ret
        }
    }
}

fn impl_arg_params(mut args: Vec<Arg>, body: Tokens) -> Tokens {
    let mut params = Vec::new();

    for arg in args.iter() {
        let name = arg.name.as_ref();
        let opt = if let Some(_) = arg.optional {
            syn::Ident::from("true")
        } else {
            syn::Ident::from("false")
        };
        params.push(
            quote! {
                pyo3::argparse::ParamDescription{name: #name, is_optional: #opt,}
            }
        );
    }
    let placeholders: Vec<syn::Ident> = params.iter().map(
        |_| syn::Ident::from("None")).collect();

    // generate extrat args
    args.reverse();
    let mut body = body;
    for arg in args.iter() {
        body = impl_arg_param(&arg, &body);
    }

    // create array of arguments, and then parse
    quote! {
        const PARAMS: &'static [pyo3::argparse::ParamDescription<'static>] = &[
            #(#params),*
        ];

        let mut output = [#(#placeholders),*];
        match pyo3::argparse::parse_args(
            py, Some(LOCATION), PARAMS, &args, kwargs.as_ref(), &mut output) {
            Ok(_) => {
                let mut _iter = output.iter();

                #body
            },
            Err(err) => Err(err)
        }
    }
}

fn impl_arg_param(arg: &Arg, body: &Tokens) -> Tokens {
    let ty = arg.ty;
    let name = arg.name;

    // First unwrap() asserts the iterated sequence is long enough (which should be guaranteed);
    // second unwrap() asserts the parameter was not missing (which fn
    // parse_args already checked for).

    if let Some(ref opt_ty) = arg.optional {
        quote! {
            match match _iter.next().unwrap().as_ref() {
                Some(obj) => {
                    match <#opt_ty as pyo3::FromPyObject>::extract(py, obj) {
                        Ok(obj) => Ok(Some(obj)),
                        Err(e) => Err(e),
                    }
                },
                None => Ok(None)
            } {
                Ok(#name) => #body,
                Err(e) => Err(e)
            }
        }
    } else {
        quote! {
            match <#ty as pyo3::FromPyObject>::extract(
                py, _iter.next().unwrap().as_ref().unwrap())
            {
                Ok(#name) => {
                    #body
                }
                Err(e) => Err(e)
            }
        }
    }
}

fn impl_py_method_def(name: &syn::Ident, wrapper: &Tokens) -> Tokens {
    quote! {{
        #wrapper

        pyo3::class::PyMethodDef {
            ml_name: stringify!(#name),
            ml_meth: pyo3::class::PyMethodType::PyCFunctionWithKeywords(wrap),
            ml_flags: pyo3::ffi::METH_VARARGS | pyo3::ffi::METH_KEYWORDS,
            ml_doc: "",
        }
    }}
}
