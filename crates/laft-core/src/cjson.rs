use std::fmt;
use std::fmt::Write as _;

use serde::Serialize;
use serde_json::{Number, Value};

/// The largest integer magnitude that survives a round trip through an IEEE
/// 754 double, `2^53 - 1`.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Why a value could not be serialized as canonical JSON.
#[derive(Debug)]
pub enum CanonicalJsonError {
    /// The value could not be represented as JSON at all, for example a map
    /// with non-string keys.
    Serialize(serde_json::Error),
    /// An integer outside `-(2^53 - 1)..=2^53 - 1`. Canonical JSON numbers are
    /// doubles, so such an integer would be rounded, and two different values
    /// could serialize to the same bytes.
    UnsafeInteger(Number),
}

impl fmt::Display for CanonicalJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "cannot serialize as JSON: {e}"),
            Self::UnsafeInteger(n) => write!(
                f,
                "integer {n} is outside the range a double represents exactly"
            ),
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
/// # Errors
///
/// Fails when the value cannot be represented as JSON, or contains an integer
/// whose magnitude exceeds `2^53 - 1`.
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
    if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > MAX_SAFE_INTEGER {
            return Err(CanonicalJsonError::UnsafeInteger(n.clone()));
        }
        // Writing to a String cannot fail.
        let _ = write!(out, "{i}");
    } else if n.is_u64() {
        return Err(CanonicalJsonError::UnsafeInteger(n.clone()));
    } else if let Some(f) = n.as_f64() {
        write_double(f, out);
    }
    Ok(())
}

/// Writes a finite double the way ECMAScript's `Number.prototype.toString`
/// does, which is the number format canonical JSON requires.
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
