use std::fmt;
use std::fmt::Write as _;

use serde::Serialize;
use serde_json::{Number, Value};

/// Why a value could not be serialized as canonical JSON.
#[derive(Debug)]
pub enum CanonicalJsonError {
    /// The value could not be represented as JSON at all, for example a map
    /// with non-string keys.
    Serialize(serde_json::Error),
    /// An integer whose digits are not how any double is canonically written,
    /// such as `2^53 + 1` or `2^60`. Canonical JSON numbers are doubles, and
    /// accepting such digits would give one double two canonical forms.
    UnsafeInteger(Number),
}

impl fmt::Display for CanonicalJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "cannot serialize as JSON: {e}"),
            Self::UnsafeInteger(n) => {
                write!(f, "integer {n} is not the canonical form of a double")
            }
        }
    }
}

impl std::error::Error for CanonicalJsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) => Some(e),
            Self::UnsafeInteger(_) => None,
        }
    }
}

/// Serializes `value` as canonical JSON (RFC 8785).
///
/// The output has no whitespace, object members sorted by the UTF-16 code
/// units of their names, strings with only the mandatory escapes, and numbers
/// in the shortest form that reads back as the same double. Equal values
/// always produce identical bytes, which makes the output safe to hash and
/// sign.
///
/// ```
/// use serde_json::json;
///
/// let bytes = laft_core::to_canonical_json(&json!({ "b": 1, "a": [true, 2.50] }))?;
/// assert_eq!(bytes, br#"{"a":[true,2.5],"b":1}"#);
/// # Ok::<(), laft_core::CanonicalJsonError>(())
/// ```
///
/// An integer is accepted when its digits are exactly how some double is
/// written, so the output of this function always canonicalizes to itself
/// again.
///
/// Non-finite floats have no JSON form. serde turns NaN and infinities into
/// `null` before this function sees them, so callers that hash floats must
/// keep them finite.
///
/// # Errors
///
/// Fails when the value cannot be represented as JSON, or contains an integer
/// whose digits are not the canonical form of a double.
pub fn to_canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalJsonError> {
    let value = serde_json::to_value(value).map_err(CanonicalJsonError::Serialize)?;
    let mut out = String::new();
    write_value(&value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_value(value: &Value, out: &mut String) -> Result<(), CanonicalJsonError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut members: Vec<_> = members.iter().collect();
            members.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, (name, item)) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(name, out);
                out.push(':');
                write_value(item, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(n: &Number, out: &mut String) -> Result<(), CanonicalJsonError> {
    let integer = n
        .as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from));
    if let Some(i) = integer {
        let text = i.to_string();
        if i.unsigned_abs() > 1 << 53 {
            let mut as_double = String::new();
            write_double(i as f64, &mut as_double);
            if as_double != text {
                return Err(CanonicalJsonError::UnsafeInteger(n.clone()));
            }
        }
        out.push_str(&text);
    } else if let Some(f) = n.as_f64() {
        write_double(f, out);
    }
    Ok(())
}

/// Writes a double in the number format canonical JSON requires, which is
/// ECMAScript's `Number.prototype.toString`.
///
/// `f` must be finite. JSON values never hold anything else.
fn write_double(f: f64, out: &mut String) {
    if f == 0.0 {
        out.push('0');
        return;
    }
    if f < 0.0 {
        out.push('-');
    }
    // `{:e}` prints the shortest digit string that reads back as the same
    // double, as `d[.ddd]e<exp>`.
    let scientific = format!("{:e}", f.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("scientific notation always contains an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent
        .parse()
        .expect("scientific notation exponent is an integer");
    let digits = prefer_even_on_tie(f.abs(), digits, exponent);
    let k = digits.len() as i32;
    let n = exponent + 1;
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(out, "e{}{}", if n > 0 { '+' } else { '-' }, (n - 1).abs());
    }
}

/// Returns the even candidate when `x` lies exactly halfway between two
/// shortest digit strings.
///
/// The shortest-digit printer resolves such ties upward, but the number format
/// of canonical JSON requires the even digit. A tie means that the exact
/// decimal expansion of `x` is the `k` shortest digits plus one more digit,
/// `5`.
fn prefer_even_on_tie(x: f64, digits: String, exponent: i32) -> String {
    // Ties are resolved upward, so only an odd last digit can be wrong.
    if digits
        .as_bytes()
        .last()
        .is_none_or(|d| (d - b'0').is_multiple_of(2))
    {
        return digits;
    }
    // 800 fractional digits exceed the longest exact expansion of any double,
    // so this prints `x` exactly.
    let exact = format!("{x:.800e}");
    let Some((mantissa, exact_exponent)) = exact.split_once('e') else {
        return digits;
    };
    if exact_exponent.parse() != Ok(exponent) {
        return digits;
    }
    let exact_digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exact_digits = exact_digits.trim_end_matches('0');
    let k = digits.len();
    if exact_digits.len() != k + 1 || !exact_digits.ends_with('5') {
        return digits;
    }
    let lower = &exact_digits[..k];
    let even = if lower.as_bytes()[k - 1].is_multiple_of(2) {
        lower.to_owned()
    } else {
        match increment_digits(lower) {
            Some(upper) => upper,
            None => return digits,
        }
    };
    let reads_back = format!("{}.{}e{exponent}", &even[..1], &even[1..])
        .parse::<f64>()
        .is_ok_and(|parsed| parsed == x);
    if reads_back { even } else { digits }
}

/// Adds one to a decimal digit string, or returns `None` when the result
/// needs an extra digit.
fn increment_digits(digits: &str) -> Option<String> {
    let mut bytes = digits.as_bytes().to_vec();
    for byte in bytes.iter_mut().rev() {
        if *byte == b'9' {
            *byte = b'0';
        } else {
            *byte += 1;
            return String::from_utf8(bytes).ok();
        }
    }
    None
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < '\u{20}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
#[path = "cjson_tests.rs"]
mod tests;
