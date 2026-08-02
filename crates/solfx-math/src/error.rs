//! Every failure mode is named. `ARCHITECTURE.md` § 13.2 forbids `unwrap`, `panic` and
//! bare boolean returns — the frontend renders these discriminants as real messages.

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    /// An intermediate exceeded u128/i128, or a result did not fit its output type.
    Overflow,
    /// Division by zero.
    DivideByZero,
    /// Oracle or computed price was <= 0.
    InvalidPrice,
    /// `conf / price` exceeded the market's ceiling.
    ConfidenceTooWide,
    /// Notional below [`crate::constants::MIN_NOTIONAL_QUOTE`].
    NotionalTooSmall,
    /// A rate, ratio or bps input was outside its permitted domain.
    InvalidParameter,
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Overflow => "arithmetic overflow",
            Self::DivideByZero => "divide by zero",
            Self::InvalidPrice => "price must be positive",
            Self::ConfidenceTooWide => "oracle confidence exceeds market ceiling",
            Self::NotionalTooSmall => "notional below minimum",
            Self::InvalidParameter => "parameter out of range",
        };
        f.write_str(s)
    }
}

pub type MathResult<T> = Result<T, MathError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant must render something a user could act on. An empty or duplicated
    /// message means the frontend cannot tell two failures apart.
    #[test]
    fn every_variant_has_a_distinct_message() {
        let all = [
            MathError::Overflow,
            MathError::DivideByZero,
            MathError::InvalidPrice,
            MathError::ConfidenceTooWide,
            MathError::NotionalTooSmall,
            MathError::InvalidParameter,
        ];
        let mut seen: Vec<String> = Vec::new();
        for e in all {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "{e:?} rendered empty");
            assert!(!seen.contains(&msg), "{e:?} duplicated the message {msg:?}");
            seen.push(msg);
        }
    }
}
