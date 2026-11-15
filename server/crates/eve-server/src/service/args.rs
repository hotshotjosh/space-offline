//! One borrowing reader for a service arm's arguments.
//!
//! An arm used to index `Request::args` and match on [`PyValue`] by hand, and
//! a dozen modules each carried their own `int_arg` / `text_arg`. [`Args`]
//! is that helper once. Two families of accessor:
//!
//! * **lenient** (`int`, `text`, `int_or_kw`, ...): `Option`, `None` for a
//!   missing or mistyped argument, so an arm keeps its own refusal text with
//!   [`Args::bad`];
//! * **strict** (`need_*`, `opt_*`): `Result<_, ArgError>`, which converts to
//!   [`ServiceError::BadArguments`] -- the wire behaviour every arm already
//!   has for "the arguments were not what the method needs".
//!
//! What is accepted [V: the helpers this replaces]: an integer is
//! [`PyValue::Int`] only (the decoder yields `Int` for every integer tag, and
//! `Long` is encode-only; a `Bool` is not an id); a string is `unicode` or
//! `str` (bytes, decoded lossily); a keyword is looked up with
//! [`crate::pyutil::pairs_get`], which knows the wire pairs are
//! `(value, key)`.

use super::{Request, ServiceError};
use crate::pyutil::pairs_get;
use eve_marshal::PyValue;

/// Why an argument was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgFault {
    /// Nothing at that position (or under that keyword).
    Missing,
    /// Present, but not the type the accessor reads.
    WrongType(&'static str),
    /// An integer that does not fit the width asked for.
    OutOfRange,
}

/// A refused argument, carrying the call it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgError {
    pub service: String,
    pub method: String,
    /// Position (`#2`) or keyword name.
    pub which: String,
    pub fault: ArgFault,
}

impl ArgError {
    fn why(&self) -> String {
        match &self.fault {
            ArgFault::Missing => format!("{} is missing", self.which),
            ArgFault::WrongType(want) => format!("{} is not {want}", self.which),
            ArgFault::OutOfRange => format!("{} is out of range", self.which),
        }
    }
}

impl From<ArgError> for ServiceError {
    fn from(e: ArgError) -> Self {
        let why = e.why();
        ServiceError::BadArguments {
            service: e.service,
            method: e.method,
            why,
        }
    }
}

/// The arguments of one call, borrowed from its [`Request`].
#[derive(Clone, Copy)]
pub struct Args<'a> {
    service: &'a str,
    method: &'a str,
    args: &'a [PyValue],
    kwargs: &'a [(PyValue, PyValue)],
}

impl<'a> Args<'a> {
    pub fn new(req: &Request<'a>) -> Self {
        Args {
            service: req.service,
            method: req.method,
            args: req.args,
            kwargs: req.kwargs,
        }
    }

    /// The refusal an arm gives for arguments that are not what it needs.
    pub fn bad(&self, why: &str) -> ServiceError {
        ServiceError::BadArguments {
            service: self.service.to_owned(),
            method: self.method.to_owned(),
            why: why.to_owned(),
        }
    }

    /// The refusal `expected <shape>` that the drone, fighter and module
    /// arms give for arguments that are not the shape the client sends.
    pub fn expected(&self, want: &str) -> ServiceError {
        self.bad(&format!("expected {want}"))
    }

    fn err(&self, which: String, fault: ArgFault) -> ArgError {
        ArgError {
            service: self.service.to_owned(),
            method: self.method.to_owned(),
            which,
            fault,
        }
    }

    /// The arguments of a call known only by their values, for a unit test of
    /// a callee that reads an [`Args`].
    #[cfg(test)]
    pub fn of(args: &'a [PyValue]) -> Self {
        Args {
            service: "svc",
            method: "Meth",
            args,
            kwargs: &[],
        }
    }

    /// This call's service and method over other values (a line of a rowset
    /// the call carried), so a refusal still names the call.
    pub fn over(&self, args: &'a [PyValue]) -> Args<'a> {
        Args { args, ..*self }
    }

    // ---- raw ---------------------------------------------------------------

    /// Positional argument `i`.
    pub fn get(&self, i: usize) -> Option<&'a PyValue> {
        self.args.get(i)
    }

    /// The keyword `name`.
    pub fn kw(&self, name: &str) -> Option<&'a PyValue> {
        pairs_get(self.kwargs, name.as_bytes())
    }

    /// Positional `i`, or the keyword `name` when there is no such position.
    pub fn get_or_kw(&self, i: usize, name: &str) -> Option<&'a PyValue> {
        self.get(i).or_else(|| self.kw(name))
    }

    /// How many positional arguments came.
    pub fn len(&self) -> usize {
        self.args.len()
    }

    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    // ---- lenient: None when missing or mistyped ------------------------------

    pub fn int(&self, i: usize) -> Option<i64> {
        as_int(self.get(i))
    }

    pub fn int_kw(&self, name: &str) -> Option<i64> {
        as_int(self.kw(name))
    }

    pub fn int_or_kw(&self, i: usize, name: &str) -> Option<i64> {
        as_int(self.get_or_kw(i, name))
    }

    /// The keyword `name`, or positional `i` when the keyword is absent or
    /// not an integer (the inventory arms: `List(flag=f)` or `List(f)`).
    pub fn int_kw_or(&self, name: &str, i: usize) -> Option<i64> {
        self.int_kw(name).or_else(|| self.int(i))
    }

    /// As [`Args::int_or_kw`], but a `Bool` reads as 0 or 1.
    pub fn int_lax_or_kw(&self, i: usize, name: &str) -> Option<i64> {
        as_int_lax(self.get_or_kw(i, name))
    }

    /// As [`Args::int`], but a `Bool` reads as 0 or 1.
    pub fn int_lax(&self, i: usize) -> Option<i64> {
        as_int_lax(self.get(i))
    }

    /// As [`Args::int_kw`], but a `Bool` reads as 0 or 1.
    pub fn int_lax_kw(&self, name: &str) -> Option<i64> {
        as_int_lax(self.kw(name))
    }

    /// As [`as_int_trunc`], positional `i`.
    pub fn int_trunc(&self, i: usize) -> Option<i64> {
        as_int_trunc(self.get(i))
    }

    /// As [`Args::float`], but a non-finite real is refused.
    pub fn float_finite(&self, i: usize) -> Option<f64> {
        as_float_finite(self.get(i))
    }

    /// As [`as_float_lax`], positional `i`: a real, an integer widened, or a
    /// `Bool` as 0.0 or 1.0.
    pub fn float_lax(&self, i: usize) -> Option<f64> {
        as_float_lax(self.get(i))
    }

    /// As [`as_float_lax_finite`], positional `i`.
    pub fn float_lax_finite(&self, i: usize) -> Option<f64> {
        as_float_lax_finite(self.get(i))
    }

    /// As [`as_int_real`], positional `i`.
    pub fn int_real(&self, i: usize) -> Option<i64> {
        as_int_real(self.get(i))
    }

    /// As [`as_id_or_ids`], positional `i`.
    pub fn id_or_ids(&self, i: usize) -> Vec<i64> {
        as_id_or_ids(self.get(i))
    }

    /// As [`Args::flag_lax_or_kw`], positional `i` only.
    pub fn flag_lax(&self, i: usize) -> bool {
        as_flag_lax(self.get(i))
    }

    /// Positional `i` as an id, or the refusal `argument i (what) is not an
    /// id` that the bookmark arms give.
    pub fn id_named(&self, i: usize, what: &str) -> Result<i64, ServiceError> {
        self.int(i)
            .ok_or_else(|| self.bad(&format!("argument {i} ({what}) is not an id")))
    }

    /// A real (an integer widened), positional `i` or keyword `name`.
    pub fn float_or_kw(&self, i: usize, name: &str) -> Option<f64> {
        as_float(self.get_or_kw(i, name))
    }

    /// As [`as_flag_lax`], positional `i` or keyword `name`.
    pub fn flag_lax_or_kw(&self, i: usize, name: &str) -> bool {
        as_flag_lax(self.get_or_kw(i, name))
    }

    pub fn text(&self, i: usize) -> Option<String> {
        as_text(self.get(i))
    }

    pub fn text_kw(&self, name: &str) -> Option<String> {
        as_text(self.kw(name))
    }

    pub fn text_or_kw(&self, i: usize, name: &str) -> Option<String> {
        as_text(self.get_or_kw(i, name))
    }

    pub fn float(&self, i: usize) -> Option<f64> {
        as_float(self.get(i))
    }

    pub fn flag(&self, i: usize) -> Option<bool> {
        as_bool(self.get(i))
    }

    pub fn list(&self, i: usize) -> Option<&'a [PyValue]> {
        as_list(self.get(i))
    }

    /// Positional `i` as a list of integers (non-integer entries skipped),
    /// or `None` when it is not a list at all.
    pub fn ids(&self, i: usize) -> Option<Vec<i64>> {
        as_list(self.get(i)).map(|l| l.iter().filter_map(|e| as_int(Some(e))).collect())
    }

    /// Positional `i` as a list of integers; a non-integer entry is skipped
    /// and anything that is not a list reads as empty.
    pub fn int_list(&self, i: usize) -> Vec<i64> {
        as_int_list(self.get(i))
    }

    // ---- strict: Result --------------------------------------------------------

    pub fn need_int(&self, i: usize) -> Result<i64, ArgError> {
        self.strict(i, "an integer", as_int)
    }

    /// An integer that must fit `i32`.
    pub fn need_i32(&self, i: usize) -> Result<i32, ArgError> {
        let n = self.need_int(i)?;
        i32::try_from(n).map_err(|_| self.err(format!("#{i}"), ArgFault::OutOfRange))
    }

    pub fn need_float(&self, i: usize) -> Result<f64, ArgError> {
        self.strict(i, "a number", as_float)
    }

    pub fn need_bool(&self, i: usize) -> Result<bool, ArgError> {
        self.strict(i, "a bool", as_bool)
    }

    pub fn need_text(&self, i: usize) -> Result<String, ArgError> {
        self.strict(i, "a string", as_text)
    }

    pub fn need_list(&self, i: usize) -> Result<&'a [PyValue], ArgError> {
        self.strict(i, "a list", as_list)
    }

    /// The keyword `name`, which must be an integer.
    pub fn need_int_kw(&self, name: &str) -> Result<i64, ArgError> {
        match self.kw(name) {
            None => Err(self.err(name.to_owned(), ArgFault::Missing)),
            Some(v) => as_int(Some(v))
                .ok_or_else(|| self.err(name.to_owned(), ArgFault::WrongType("an integer"))),
        }
    }

    /// An optional integer: absent or `None` is `Ok(None)`, a mistyped value
    /// is an error.
    pub fn opt_int(&self, i: usize) -> Result<Option<i64>, ArgError> {
        match self.get(i) {
            None | Some(PyValue::None) => Ok(None),
            Some(v) => as_int(Some(v))
                .map(Some)
                .ok_or_else(|| self.err(format!("#{i}"), ArgFault::WrongType("an integer"))),
        }
    }

    /// An optional string, as [`Args::opt_int`].
    pub fn opt_text(&self, i: usize) -> Result<Option<String>, ArgError> {
        match self.get(i) {
            None | Some(PyValue::None) => Ok(None),
            Some(v) => as_text(Some(v))
                .map(Some)
                .ok_or_else(|| self.err(format!("#{i}"), ArgFault::WrongType("a string"))),
        }
    }

    fn strict<T>(
        &self,
        i: usize,
        want: &'static str,
        read: impl Fn(Option<&'a PyValue>) -> Option<T>,
    ) -> Result<T, ArgError> {
        match self.get(i) {
            None => Err(self.err(format!("#{i}"), ArgFault::Missing)),
            Some(v) => {
                read(Some(v)).ok_or_else(|| self.err(format!("#{i}"), ArgFault::WrongType(want)))
            }
        }
    }
}

/// An integer value read off a [`PyValue`] already in hand.
pub fn as_int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// An integer, or a `Bool` read as 0 or 1 (what corporation and notification
/// arms have always accepted for a flag-like id).
pub fn as_int_lax(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// A flag the client sends as a `Bool` or an integer: nonzero is true,
/// anything else (including nothing) is false.
pub fn as_flag_lax(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    }
}

/// `str` or `unicode`; bytes are decoded lossily.
pub fn as_text(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

/// A real, or an integer widened.
pub fn as_float(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(f) => Some(*f),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// A real; an integer is widened and a `Bool` reads as 0.0 or 1.0 (what the
/// probe scanner takes for a coordinate or an angle).
pub fn as_float_lax(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// An integer; a `Bool` reads as 0 or 1 and a finite real is truncated
/// toward zero (what the market arms take for an id or a count, since the
/// client's `KeyVal` fields may arrive as either).
pub fn as_int_trunc(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        PyValue::Real(r) if r.is_finite() => Some(*r as i64),
        _ => None,
    }
}

/// An integer, or a finite real truncated toward zero; a `Bool` is refused
/// (what the movement commands take for a ball id).
pub fn as_int_real(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Real(f) if f.is_finite() => Some(*f as i64),
        _ => None,
    }
}

/// As [`as_float_lax`], but a non-finite real is `None` (what the movement
/// commands take for a heading, a range or a fraction).
pub fn as_float_lax_finite(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(f) => f.is_finite().then_some(*f),
        other => as_float_lax(Some(other)),
    }
}

/// As [`as_float`], but a non-finite real (NaN, infinity) is `None`; an
/// integer is widened.
pub fn as_float_finite(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(f) if f.is_finite() => Some(*f),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// The integers of a list or tuple, skipping other entries; empty for
/// anything that is not a list.
pub fn as_int_list(v: Option<&PyValue>) -> Vec<i64> {
    as_list(v).map_or_else(Vec::new, |l| {
        l.iter().filter_map(|e| as_int(Some(e))).collect()
    })
}

/// A bare integer as a list of one, or the integers of a list or tuple
/// (other entries skipped); empty for anything else. Read by the arms that
/// take "an id or a list of ids" (`MultiAdd`, `GetGroupsMany`,
/// `CheckMyDockingAccessToStructures`, `GetMultiAllianceShortNamesEx` and the
/// like).
pub fn as_id_or_ids(v: Option<&PyValue>) -> Vec<i64> {
    as_int(v).map_or_else(|| as_int_list(v), |n| vec![n])
}

/// An integer, or a real with no fractional part read as one (a skill-point
/// count may arrive as either); `None` for a fractional or non-finite real
/// and for anything else. Read by the free-skill-point and injector arms.
pub fn as_int_whole(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(i)) => Some(*i),
        Some(PyValue::Real(r)) if r.fract() == 0.0 => Some(*r as i64),
        _ => None,
    }
}

/// A list of integers where every entry must be one; a bare integer is a
/// list of one. `None` for any other shape.
pub fn as_id_list_strict(v: Option<&PyValue>) -> Option<Vec<i64>> {
    match v? {
        PyValue::Int(id) => Some(vec![*id]),
        PyValue::List(items) | PyValue::Tuple(items) => {
            items.iter().map(|i| as_int(Some(i))).collect()
        }
        _ => None,
    }
}

fn as_bool(v: Option<&PyValue>) -> Option<bool> {
    match v? {
        PyValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// A list or tuple.
pub fn as_list(v: Option<&PyValue>) -> Option<&[PyValue]> {
    match v? {
        PyValue::List(l) | PyValue::Tuple(l) => Some(l),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(name: &str, v: PyValue) -> (PyValue, PyValue) {
        // wire pairs are (value, key)
        (v, PyValue::Bytes(name.as_bytes().to_vec()))
    }

    fn with<R>(
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
        f: impl FnOnce(Args<'_>) -> R,
    ) -> R {
        let req = Request {
            service: "svc",
            method: "Meth",
            args,
            kwargs,
            bind_params: None,
            actor: None,
        };
        f(Args::new(&req))
    }

    #[test]
    fn int_reads_only_int() {
        let a = [
            PyValue::Int(7),
            PyValue::Bool(true),
            PyValue::Unicode("7".into()),
            PyValue::Long(7),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.int(0), Some(7));
            assert_eq!(x.int(1), None);
            assert_eq!(x.int(2), None);
            assert_eq!(x.int(3), None);
            assert_eq!(x.int(4), None);
        });
    }

    #[test]
    fn text_reads_str_and_unicode() {
        let a = [
            PyValue::Unicode("ab".into()),
            PyValue::Bytes(b"cd".to_vec()),
            PyValue::Int(1),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.text(0).as_deref(), Some("ab"));
            assert_eq!(x.text(1).as_deref(), Some("cd"));
            assert_eq!(x.text(2), None);
            assert_eq!(x.text(3), None);
        });
    }

    #[test]
    fn float_bool_and_list() {
        let a = [
            PyValue::Real(1.5),
            PyValue::Int(2),
            PyValue::Bool(true),
            PyValue::List(vec![PyValue::Int(1)]),
            PyValue::Tuple(vec![]),
            PyValue::None,
        ];
        with(&a, &[], |x| {
            assert_eq!(x.float(0), Some(1.5));
            assert_eq!(x.float(1), Some(2.0));
            assert_eq!(x.float(2), None);
            assert_eq!(x.flag(2), Some(true));
            assert_eq!(x.flag(0), None);
            assert_eq!(x.list(3).map(<[_]>::len), Some(1));
            assert_eq!(x.list(4).map(<[_]>::len), Some(0));
            assert_eq!(x.list(0), None);
            assert_eq!(x.list(9), None);
            assert_eq!(x.len(), 6);
            assert!(!x.is_empty());
        });
    }

    #[test]
    fn lax_readers_and_keyword_forms() {
        let a = [
            PyValue::Bool(true),
            PyValue::Int(0),
            PyValue::Int(4),
            PyValue::Unicode("x".into()),
            PyValue::Real(1.5),
        ];
        let kw = [kv("k", PyValue::Int(3)), kv("f", PyValue::Bool(true))];
        with(&a, &kw, |x| {
            assert_eq!(as_int_lax(x.get(0)), Some(1));
            assert_eq!(as_int_lax(x.get(3)), None);
            assert_eq!(as_int_lax(None), None);
            assert_eq!(x.int_lax_or_kw(0, "z"), Some(1));
            assert_eq!(x.int_lax_or_kw(9, "k"), Some(3));
            assert_eq!(x.int_lax_or_kw(3, "z"), None);
            assert!(as_flag_lax(x.get(0)));
            assert!(!as_flag_lax(x.get(1)));
            assert!(as_flag_lax(x.get(2)));
            assert!(!as_flag_lax(x.get(3)));
            assert!(!as_flag_lax(None));
            assert!(x.flag_lax_or_kw(9, "f"));
            assert!(!x.flag_lax_or_kw(9, "z"));
            assert_eq!(x.float_or_kw(4, "z"), Some(1.5));
            assert_eq!(x.float_or_kw(2, "z"), Some(4.0));
            assert_eq!(x.float_or_kw(9, "k"), Some(3.0));
            assert_eq!(x.float_or_kw(3, "z"), None);
        });
    }

    #[test]
    fn lax_positional_and_named_id() {
        let a = [PyValue::Bool(true), PyValue::Int(0), PyValue::Int(8)];
        let kw = [kv("k", PyValue::Int(-3)), kv("f", PyValue::Real(f64::NAN))];
        with(&a, &kw, |x| {
            assert!(x.flag_lax(0));
            assert!(!x.flag_lax(1));
            assert!(!x.flag_lax(9));
            assert_eq!(x.int_lax(0), Some(1));
            assert_eq!(x.int_lax(2), Some(8));
            assert_eq!(x.int_lax(9), None);
            assert_eq!(x.int_lax_kw("k"), Some(-3));
            assert_eq!(x.int_lax_kw("z"), None);
            assert_eq!(x.id_named(2, "itemID"), Ok(8));
            assert_eq!(
                x.id_named(0, "itemID"),
                Err(x.bad("argument 0 (itemID) is not an id"))
            );
        });
    }

    #[test]
    fn finite_floats_refuse_nan_and_infinity() {
        let a = [
            PyValue::Real(1.5),
            PyValue::Int(2),
            PyValue::Real(f64::NAN),
            PyValue::Real(f64::INFINITY),
            PyValue::Bool(true),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.float_finite(0), Some(1.5));
            assert_eq!(x.float_finite(1), Some(2.0));
            assert_eq!(x.float_finite(2), None);
            assert_eq!(x.float_finite(3), None);
            assert_eq!(x.float_finite(4), None);
            assert_eq!(x.int_trunc(0), Some(1));
            assert_eq!(x.int_trunc(1), Some(2));
            assert_eq!(x.int_trunc(2), None);
            assert_eq!(x.int_trunc(3), None);
            assert_eq!(x.int_trunc(4), Some(1));
            assert_eq!(as_int_trunc(Some(&PyValue::Real(-2.9))), Some(-2));
            assert_eq!(as_float_lax(x.get(4)), Some(1.0));
            assert_eq!(as_float_lax(x.get(1)), Some(2.0));
            assert_eq!(as_float_lax(x.get(0)), Some(1.5));
            assert_eq!(as_float_lax(Some(&PyValue::Unicode("1".into()))), None);
            assert_eq!(as_float_lax(None), None);
            assert_eq!(
                as_list(Some(&PyValue::Tuple(vec![]))).map(<[_]>::len),
                Some(0)
            );
        });
    }

    #[test]
    fn movement_readers_and_ids() {
        let a = [
            PyValue::Real(2.9),
            PyValue::Real(f64::NAN),
            PyValue::Bool(true),
            PyValue::Int(-4),
            PyValue::List(vec![PyValue::Int(1), PyValue::Real(1.0), PyValue::Int(2)]),
            PyValue::Int(5),
        ];
        with(&a, &[], |x| {
            assert_eq!(as_int_real(x.get(0)), Some(2));
            assert_eq!(as_int_real(x.get(1)), None);
            assert_eq!(as_int_real(x.get(2)), None);
            assert_eq!(as_int_real(x.get(3)), Some(-4));
            assert_eq!(as_int_real(None), None);
            assert_eq!(as_float_lax_finite(x.get(0)), Some(2.9));
            assert_eq!(as_float_lax_finite(x.get(1)), None);
            assert_eq!(as_float_lax_finite(x.get(2)), Some(1.0));
            assert_eq!(as_float_lax_finite(x.get(3)), Some(-4.0));
            assert_eq!(as_float_lax_finite(None), None);
            assert_eq!(x.ids(4), Some(vec![1, 2]));
            assert_eq!(x.ids(5), None);
            assert_eq!(x.ids(9), None);
            assert_eq!(x.int_kw_or("nope", 3), Some(-4));
            assert_eq!(x.int_kw_or("nope", 2), None);
            assert_eq!(x.expected("(a,)"), x.bad("expected (a,)"));
        });
    }

    #[test]
    fn int_lists() {
        let a = [
            PyValue::List(vec![PyValue::Int(1), PyValue::Bool(true), PyValue::Int(3)]),
            PyValue::Tuple(vec![PyValue::Int(4)]),
            PyValue::Int(5),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.int_list(0), vec![1, 3]);
            assert_eq!(x.int_list(1), vec![4]);
            assert!(x.int_list(2).is_empty());
            assert!(x.int_list(9).is_empty());
            assert_eq!(as_id_list_strict(x.get(0)), None);
            assert_eq!(as_id_list_strict(x.get(1)), Some(vec![4]));
            assert_eq!(as_id_list_strict(x.get(2)), Some(vec![5]));
            assert_eq!(as_id_list_strict(None), None);
        });
    }

    #[test]
    fn keyword_present_absent_and_positional_wins() {
        let kw = [kv("allianceID", PyValue::Int(9))];
        with(&[], &kw, |x| {
            assert_eq!(x.int_kw("allianceID"), Some(9));
            assert_eq!(x.int_kw("other"), None);
            assert_eq!(x.int_or_kw(0, "allianceID"), Some(9));
            assert_eq!(x.int_or_kw(0, "other"), None);
            assert_eq!(x.need_int_kw("allianceID"), Ok(9));
            assert!(matches!(
                x.need_int_kw("other"),
                Err(ArgError {
                    fault: ArgFault::Missing,
                    ..
                })
            ));
        });
        let a = [PyValue::Int(1)];
        with(&a, &kw, |x| {
            assert_eq!(x.int_or_kw(0, "allianceID"), Some(1))
        });
        let kw = [kv("n", PyValue::Unicode("x".into()))];
        with(&[], &kw, |x| {
            assert_eq!(x.text_or_kw(0, "n").as_deref(), Some("x"));
            assert_eq!(x.text_kw("n").as_deref(), Some("x"));
            assert!(matches!(
                x.need_int_kw("n"),
                Err(ArgError {
                    fault: ArgFault::WrongType(_),
                    ..
                })
            ));
        });
    }

    #[test]
    fn need_accessors_succeed() {
        let a = [
            PyValue::Int(5),
            PyValue::Real(0.5),
            PyValue::Bool(false),
            PyValue::Unicode("s".into()),
            PyValue::List(vec![]),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.need_int(0), Ok(5));
            assert_eq!(x.need_i32(0), Ok(5));
            assert_eq!(x.need_float(1), Ok(0.5));
            assert_eq!(x.need_bool(2), Ok(false));
            assert_eq!(x.need_text(3), Ok("s".to_owned()));
            assert_eq!(x.need_list(4).map(<[_]>::len), Ok(0));
        });
    }

    #[test]
    fn need_accessors_fail_missing_and_wrong_type() {
        let a = [PyValue::Unicode("s".into())];
        with(&a, &[], |x| {
            let missing = |r: Result<(), ArgError>| {
                matches!(
                    r,
                    Err(ArgError {
                        fault: ArgFault::Missing,
                        ..
                    })
                )
            };
            let wrong = |r: Result<(), ArgError>| {
                matches!(
                    r,
                    Err(ArgError {
                        fault: ArgFault::WrongType(_),
                        ..
                    })
                )
            };
            assert!(wrong(x.need_int(0).map(drop)));
            assert!(wrong(x.need_i32(0).map(drop)));
            assert!(wrong(x.need_float(0).map(drop)));
            assert!(wrong(x.need_bool(0).map(drop)));
            assert!(wrong(x.need_list(0).map(drop)));
            assert!(missing(x.need_int(1).map(drop)));
            assert!(missing(x.need_i32(1).map(drop)));
            assert!(missing(x.need_float(1).map(drop)));
            assert!(missing(x.need_bool(1).map(drop)));
            assert!(missing(x.need_text(1).map(drop)));
            assert!(missing(x.need_list(1).map(drop)));
        });
    }

    #[test]
    fn i32_range_is_checked() {
        let a = [
            PyValue::Int(i64::from(i32::MAX)),
            PyValue::Int(i64::from(i32::MAX) + 1),
            PyValue::Int(i64::from(i32::MIN) - 1),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.need_i32(0), Ok(i32::MAX));
            for i in [1, 2] {
                assert!(matches!(
                    x.need_i32(i),
                    Err(ArgError {
                        fault: ArgFault::OutOfRange,
                        ..
                    })
                ));
            }
        });
    }

    #[test]
    fn optional_accessors() {
        let a = [
            PyValue::None,
            PyValue::Int(3),
            PyValue::Unicode("t".into()),
            PyValue::Bool(true),
        ];
        with(&a, &[], |x| {
            assert_eq!(x.opt_int(0), Ok(None));
            assert_eq!(x.opt_int(1), Ok(Some(3)));
            assert_eq!(x.opt_int(9), Ok(None));
            assert!(x.opt_int(2).is_err());
            assert_eq!(x.opt_text(0), Ok(None));
            assert_eq!(x.opt_text(2), Ok(Some("t".to_owned())));
            assert_eq!(x.opt_text(9), Ok(None));
            assert!(x.opt_text(3).is_err());
        });
    }

    #[test]
    fn error_maps_to_bad_arguments() {
        with(&[], &[], |x| {
            let e: ServiceError = x.need_int(2).unwrap_err().into();
            assert_eq!(
                e,
                ServiceError::BadArguments {
                    service: "svc".into(),
                    method: "Meth".into(),
                    why: "#2 is missing".into(),
                }
            );
            assert_eq!(
                x.bad("(a, b)"),
                ServiceError::BadArguments {
                    service: "svc".into(),
                    method: "Meth".into(),
                    why: "(a, b)".into(),
                }
            );
        });
    }

    #[test]
    fn id_or_ids_takes_a_bare_int_a_list_or_a_tuple() {
        use PyValue::{Bool, Int, List, Real, Tuple};
        assert_eq!(as_id_or_ids(Some(&Int(5))), vec![5]);
        let mixed = List(vec![Int(1), Bool(true), Real(2.0), Int(3)]);
        assert_eq!(as_id_or_ids(Some(&mixed)), vec![1, 3]);
        assert_eq!(as_id_or_ids(Some(&Tuple(vec![Int(4)]))), vec![4]);
        for other in [Real(5.0), Bool(true), PyValue::None] {
            assert_eq!(as_id_or_ids(Some(&other)), Vec::<i64>::new(), "{other:?}");
        }
        assert_eq!(as_id_or_ids(None), Vec::<i64>::new());
    }

    #[test]
    fn int_whole_reads_a_whole_real_and_refuses_a_fraction() {
        use PyValue::{Bool, Int, Real};
        assert_eq!(as_int_whole(Some(&Int(7))), Some(7));
        assert_eq!(as_int_whole(Some(&Real(7.0))), Some(7));
        assert_eq!(as_int_whole(Some(&Real(7.5))), None);
        assert_eq!(as_int_whole(Some(&Real(f64::NAN))), None);
        assert_eq!(as_int_whole(Some(&Bool(true))), None);
        assert_eq!(as_int_whole(None), None);
    }
}
