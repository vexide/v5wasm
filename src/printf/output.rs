//! Various ways to output formatting data.

use core::cell::Cell;
use core::fmt;
use core::str::from_utf8;
use std::ffi::*;

use wasmtime::AsContext;

use super::{Argument, DoubleFormat, Flags, Specifier, WasmVaList};

struct DummyWriter(usize);

impl fmt::Write for DummyWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 += s.len();
        Ok(())
    }
}

struct WriteCounter<'a, T: fmt::Write>(&'a mut T, usize);

impl<'a, T: fmt::Write> fmt::Write for WriteCounter<'a, T> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.1 += s.len();
        self.0.write_str(s)
    }
}

fn write_str(
    w: &mut impl fmt::Write,
    flags: Flags,
    width: c_int,
    precision: Option<c_int>,
    b: &[u8],
) -> fmt::Result {
    let string = from_utf8(b).map_err(|_| fmt::Error)?;
    let precision = precision.unwrap_or(string.len() as c_int);
    if flags.contains(Flags::LEFT_ALIGN) {
        write!(
            w,
            "{:1$.prec$}",
            string,
            width as usize,
            prec = precision as usize
        )
    } else {
        write!(
            w,
            "{:>1$.prec$}",
            string,
            width as usize,
            prec = precision as usize
        )
    }
}

macro_rules! define_numeric {
    ($w: expr, $data: expr, $flags: expr, $width: expr, $precision: expr) => {
        define_numeric!($w, $data, $flags, $width, $precision, "")
    };
    ($w: expr, $data: expr, $flags: expr, $width: expr, $precision: expr, $ty:expr) => {{
        use fmt::Write;
        if $flags.contains(Flags::LEFT_ALIGN) {
            if $flags.contains(Flags::PREPEND_PLUS) {
                write!(
                    $w,
                    concat!("{:<+width$.prec$", $ty, "}"),
                    $data,
                    width = $width as usize,
                    prec = $precision as usize
                )
            } else if $flags.contains(Flags::PREPEND_SPACE) && !$data.is_sign_negative() {
                write!(
                    $w,
                    concat!(" {:<width$.prec$", $ty, "}"),
                    $data,
                    width = ($width as usize).wrapping_sub(1),
                    prec = $precision as usize
                )
            } else {
                write!(
                    $w,
                    concat!("{:<width$.prec$", $ty, "}"),
                    $data,
                    width = $width as usize,
                    prec = $precision as usize
                )
            }
        } else if $flags.contains(Flags::PREPEND_PLUS) {
            if $flags.contains(Flags::PREPEND_ZERO) {
                write!(
                    $w,
                    concat!("{:+0width$.prec$", $ty, "}"),
                    $data,
                    width = $width as usize,
                    prec = $precision as usize
                )
            } else {
                write!(
                    $w,
                    concat!("{:+width$.prec$", $ty, "}"),
                    $data,
                    width = $width as usize,
                    prec = $precision as usize
                )
            }
        } else if $flags.contains(Flags::PREPEND_ZERO) {
            if $flags.contains(Flags::PREPEND_SPACE) && !$data.is_sign_negative() {
                let mut d = DummyWriter(0);
                let _ = write!(
                    d,
                    concat!("{:.prec$", $ty, "}"),
                    $data,
                    prec = $precision as usize
                );
                if d.0 + 1 > $width as usize {
                    $width += 1;
                }
                write!(
                    $w,
                    concat!(" {:0width$.prec$", $ty, "}"),
                    $data,
                    width = ($width as usize).wrapping_sub(1),
                    prec = $precision as usize
                )
            } else {
                write!(
                    $w,
                    concat!("{:0width$.prec$", $ty, "}"),
                    $data,
                    width = $width as usize,
                    prec = $precision as usize
                )
            }
        } else {
            if $flags.contains(Flags::PREPEND_SPACE) && !$data.is_sign_negative() {
                let mut d = DummyWriter(0);
                let _ = write!(
                    d,
                    concat!("{:.prec$", $ty, "}"),
                    $data,
                    prec = $precision as usize
                );
                if d.0 + 1 > $width as usize {
                    $width = d.0 as i32 + 1;
                }
            }
            write!(
                $w,
                concat!("{:width$.prec$", $ty, "}"),
                $data,
                width = $width as usize,
                prec = $precision as usize
            )
        }
    }};
}

macro_rules! define_unumeric {
    ($w: expr, $data: expr, $flags: expr, $width: expr, $precision: expr) => {
        define_unumeric!($w, $data, $flags, $width, $precision, "")
    };
    ($w: expr, $data: expr, $flags: expr, $width: expr, $precision: expr, $ty:expr) => {{
        if $flags.contains(Flags::LEFT_ALIGN) {
            if $flags.contains(Flags::ALTERNATE_FORM) {
                write!(
                    $w,
                    concat!("{:<#width$", $ty, "}"),
                    $data,
                    width = $width as usize
                )
            } else {
                write!(
                    $w,
                    concat!("{:<width$", $ty, "}"),
                    $data,
                    width = $width as usize
                )
            }
        } else if $flags.contains(Flags::ALTERNATE_FORM) {
            if $flags.contains(Flags::PREPEND_ZERO) {
                write!(
                    $w,
                    concat!("{:#0width$", $ty, "}"),
                    $data,
                    width = $width as usize
                )
            } else {
                write!(
                    $w,
                    concat!("{:#width$", $ty, "}"),
                    $data,
                    width = $width as usize
                )
            }
        } else if $flags.contains(Flags::PREPEND_ZERO) {
            write!(
                $w,
                concat!("{:0width$", $ty, "}"),
                $data,
                width = $width as usize
            )
        } else {
            write!(
                $w,
                concat!("{:width$", $ty, "}"),
                $data,
                width = $width as usize
            )
        }
    }};
}

/// Write to a struct that implements [`fmt::Write`].
///
/// # Differences
///
/// There are a few differences from standard printf format:
///
/// - only valid UTF-8 data can be printed.
/// - an `X` format specifier with a `#` flag prints the hex data in uppercase,
///   but the leading `0x` is still lowercase
/// - an `o` format specifier with a `#` flag precedes the number with an `o`
///   instead of `0`
/// - `g`/`G` (shorted floating point) is aliased to `f`/`F`` (decimal floating
///   point)
/// - same for `a`/`A` (hex floating point)
/// - the `n` format specifier, [`Specifier::WriteBytesWritten`], is not
///   implemented and will cause an error if encountered.
pub fn fmt_write(w: &mut impl fmt::Write) -> impl FnMut(Argument) -> c_int + '_ {
    use fmt::Write;
    move |Argument {
              flags,
              mut width,
              precision,
              specifier,
          }| {
        let mut w = WriteCounter(w, 0);
        let w = &mut w;
        let res = match specifier {
            Specifier::Percent => w.write_char('%'),
            Specifier::Bytes(data) => write_str(w, flags, width, precision, data),
            Specifier::String(data) => write_str(w, flags, width, precision, data.to_bytes()),
            Specifier::Hex(data) => {
                define_unumeric!(w, data, flags, width, precision.unwrap_or(0), "x")
            }
            Specifier::UpperHex(data) => {
                define_unumeric!(w, data, flags, width, precision.unwrap_or(0), "X")
            }
            Specifier::Octal(data) => {
                define_unumeric!(w, data, flags, width, precision.unwrap_or(0), "o")
            }
            Specifier::Uint(data) => {
                define_unumeric!(w, data, flags, width, precision.unwrap_or(0))
            }
            Specifier::Int(data) => define_numeric!(w, data, flags, width, precision.unwrap_or(0)),
            Specifier::Double { value, format } => match format {
                DoubleFormat::Normal
                | DoubleFormat::UpperNormal
                | DoubleFormat::Auto
                | DoubleFormat::UpperAuto
                | DoubleFormat::Hex
                | DoubleFormat::UpperHex => {
                    define_numeric!(w, value, flags, width, precision.unwrap_or(6))
                }
                DoubleFormat::Scientific => {
                    define_numeric!(w, value, flags, width, precision.unwrap_or(6), "e")
                }
                DoubleFormat::UpperScientific => {
                    define_numeric!(w, value, flags, width, precision.unwrap_or(6), "E")
                }
            },
            Specifier::Char(data) => {
                if flags.contains(Flags::LEFT_ALIGN) {
                    write!(w, "{:width$}", data as char, width = width as usize)
                } else {
                    write!(w, "{:>width$}", data as char, width = width as usize)
                }
            }
            Specifier::Pointer(data) => {
                if flags.contains(Flags::LEFT_ALIGN) {
                    write!(
                        w,
                        "{:<width$p}",
                        data as *const c_void,
                        width = width as usize
                    )
                } else if flags.contains(Flags::PREPEND_ZERO) {
                    write!(
                        w,
                        "{:0width$p}",
                        data as *const c_void,
                        width = width as usize
                    )
                } else {
                    write!(
                        w,
                        "{:width$p}",
                        data as *const c_void,
                        width = width as usize
                    )
                }
            }
            Specifier::WriteBytesWritten(_, _) => Err(Default::default()),
        };
        match res {
            Ok(_) => w.1 as c_int,
            Err(_) => -1,
        }
    }
}

/// Returns an object that implements [`Display`][fmt::Display] for safely
/// printing formatting data. This is slightly less performant than using
/// [`fmt_write`], but may be the only option.
///
/// This shares the same caveats as [`fmt_write`].
pub fn display<T: AsContext>(
    format: CString,
    va_list: WasmVaList,
    ctx: &T,
) -> VaListDisplay<'_, T> {
    VaListDisplay {
        format,
        va_list,
        ctx,
        written: Cell::new(0),
    }
}

/// Helper struct created by [`display`] for safely printing `printf`-style
/// formatting with [`format!`] and `{}`. This can be used with anything that
/// uses [`format_args!`], such as [`println!`] or the `log` crate.
///
/// ```rust
/// #![feature(c_variadic)]
///
/// use cty::{c_char, c_int};
///
/// #[no_mangle]
/// unsafe extern "C" fn c_library_print(str: *const c_char, mut args: ...) -> c_int {
///     let format = printf_compat::output::display(str, args.as_va_list());
///     println!("{}", format);
///     format.bytes_written()
/// }
/// ```
///
/// If you have access to [`std`], i.e. not an embedded platform, you can use
/// [`std::os::raw`] instead of [`cty`].
pub struct VaListDisplay<'a, T: AsContext> {
    format: CString,
    va_list: WasmVaList,
    ctx: &'a T,
    written: Cell<c_int>,
}

impl<T: AsContext> VaListDisplay<'_, T> {
    /// Get the number of bytes written, or 0 if there was an error.
    pub fn bytes_written(&self) -> c_int {
        self.written.get()
    }
}

impl<'a, T: AsContext> fmt::Display for VaListDisplay<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = super::format(
            self.format.as_bytes(),
            self.va_list.clone(),
            self.ctx,
            fmt_write(f),
        );
        self.written.set(bytes);
        if bytes < 0 {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}
