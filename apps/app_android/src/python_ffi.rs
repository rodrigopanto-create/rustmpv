//! Integración opcional con Python C-API para ejecutar yt-dlp dentro de Android.

use std::ffi::{CString, c_char, c_int, c_void};
use std::ptr;

type PyObject = c_void;

unsafe extern "C" {
    fn Py_Initialize();
    fn Py_IsInitialized() -> c_int;
    fn Py_FinalizeEx() -> c_int;
    fn PyErr_Occurred() -> *mut PyObject;
    fn PyErr_Fetch(
        type_: *mut *mut PyObject,
        value: *mut *mut PyObject,
        traceback: *mut *mut PyObject,
    );
    fn PyErr_Clear();
    fn PyImport_ImportModule(name: *const c_char) -> *mut PyObject;
    fn PyObject_GetAttrString(object: *mut PyObject, name: *const c_char) -> *mut PyObject;
    fn PyObject_CallObject(callable: *mut PyObject, args: *mut PyObject) -> *mut PyObject;
    fn PyObject_Call(
        callable: *mut PyObject,
        args: *mut PyObject,
        kwargs: *mut PyObject,
    ) -> *mut PyObject;
    fn PyTuple_New(size: isize) -> *mut PyObject;
    fn PyTuple_SetItem(tuple: *mut PyObject, index: isize, item: *mut PyObject) -> c_int;
    fn PyDict_New() -> *mut PyObject;
    fn PyDict_SetItemString(dict: *mut PyObject, key: *const c_char, value: *mut PyObject)
    -> c_int;
    fn PyDict_GetItemString(dict: *mut PyObject, key: *const c_char) -> *mut PyObject;
    fn PyBool_FromLong(value: isize) -> *mut PyObject;
    fn PyRun_SimpleString(command: *const c_char) -> c_int;
    fn PyUnicode_FromString(value: *const c_char) -> *mut PyObject;
    fn PyUnicode_AsUTF8(value: *mut PyObject) -> *const c_char;
    fn PyObject_Repr(object: *mut PyObject) -> *mut PyObject;
    fn Py_DecRef(object: *mut PyObject);
}

/// Inicializa Python y devuelve una descripción C-API si falla.
pub fn initialize(python_root: &str) -> Result<(), String> {
    unsafe {
        if Py_IsInitialized() == 0 {
            Py_Initialize();
        }
        if Py_IsInitialized() == 0 {
            return Err("Py_Initialize no pudo iniciar Python".to_owned());
        }
    }
    let script = format!(
        "import sys; sys.path.insert(0, r'{}'); import site; site.main()",
        python_root.replace('\'', "\\'")
    );
    let script = CString::new(script).map_err(|error| error.to_string())?;
    unsafe {
        if PyRun_SimpleString(script.as_ptr()) != 0 {
            return Err(fetch_error());
        }
    }
    Ok(())
}

/// Ejecuta yt-dlp.YoutubeDL.extract_info y devuelve info['url'].
pub fn resolve(url: &str) -> Result<String, String> {
    let module_name = CString::new("yt_dlp").map_err(|error| error.to_string())?;
    let class_name = CString::new("YoutubeDL").map_err(|error| error.to_string())?;
    let method_name = CString::new("extract_info").map_err(|error| error.to_string())?;
    let url_value = CString::new(url).map_err(|error| error.to_string())?;
    unsafe {
        let module = PyImport_ImportModule(module_name.as_ptr());
        if module.is_null() {
            return Err(fetch_error());
        }
        let class = PyObject_GetAttrString(module, class_name.as_ptr());
        Py_DecRef(module);
        if class.is_null() {
            return Err(fetch_error());
        }
        let options = PyDict_New();
        let format_key = CString::new("format").unwrap();
        let format_value = PyUnicode_FromString(c"best".as_ptr());
        let _ = PyDict_SetItemString(options, format_key.as_ptr(), format_value);
        Py_DecRef(format_value);
        let quiet_key = CString::new("quiet").unwrap();
        let quiet_value = PyBool_FromLong(1);
        let _ = PyDict_SetItemString(options, quiet_key.as_ptr(), quiet_value);
        Py_DecRef(quiet_value);
        let args = PyTuple_New(1);
        PyTuple_SetItem(args, 0, options);
        let ydl = PyObject_CallObject(class, args);
        Py_DecRef(args);
        Py_DecRef(class);
        if ydl.is_null() {
            return Err(fetch_error());
        }
        let info_method = PyObject_GetAttrString(ydl, method_name.as_ptr());
        let info_args = PyTuple_New(1);
        PyTuple_SetItem(info_args, 0, PyUnicode_FromString(url_value.as_ptr()));
        let kwargs = PyDict_New();
        let download_key = CString::new("download").unwrap();
        let false_value = PyBool_FromLong(0);
        let _ = PyDict_SetItemString(kwargs, download_key.as_ptr(), false_value);
        Py_DecRef(false_value);
        let info = PyObject_Call(info_method, info_args, kwargs);
        Py_DecRef(info_args);
        Py_DecRef(kwargs);
        Py_DecRef(info_method);
        Py_DecRef(ydl);
        if info.is_null() {
            return Err(fetch_error());
        }
        let stream = PyDict_GetItemString(info, c"url".as_ptr());
        if stream.is_null() {
            Py_DecRef(info);
            return Err(fetch_error());
        }
        // Borrowed reference: copy the URL before releasing the info dictionary.
        let result = if PyUnicode_AsUTF8(stream).is_null() {
            Err(fetch_error())
        } else {
            let url = std::ffi::CStr::from_ptr(PyUnicode_AsUTF8(stream))
                .to_string_lossy()
                .into_owned();
            Ok(url)
        };
        Py_DecRef(info);
        result
    }
}

/// Convierte la excepción Python pendiente en un mensaje Rust.
fn fetch_error() -> String {
    unsafe {
        if PyErr_Occurred().is_null() {
            return "Error desconocido de Python".to_owned();
        }
        let mut type_ = ptr::null_mut();
        let mut value = ptr::null_mut();
        let mut traceback = ptr::null_mut();
        PyErr_Fetch(&mut type_, &mut value, &mut traceback);
        let message = if !value.is_null() {
            let repr = PyObject_Repr(value);
            if !repr.is_null() {
                let text = PyUnicode_AsUTF8(repr);
                let result = if text.is_null() {
                    "Python exception".to_owned()
                } else {
                    std::ffi::CStr::from_ptr(text)
                        .to_string_lossy()
                        .into_owned()
                };
                Py_DecRef(repr);
                result
            } else {
                "Python exception".to_owned()
            }
        } else {
            "Python exception".to_owned()
        };
        for object in [type_, value, traceback] {
            if !object.is_null() {
                Py_DecRef(object);
            }
        }
        PyErr_Clear();
        message
    }
}

/// Finaliza el intérprete; normalmente se deja vivo hasta el cierre del proceso.
pub fn finalize() {
    unsafe {
        if Py_IsInitialized() != 0 {
            let _ = Py_FinalizeEx();
        }
    }
}
