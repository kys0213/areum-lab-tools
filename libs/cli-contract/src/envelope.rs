//! The `--json` envelope. Field order (`ok`, `command`, then `data` or
//! `error`) is part of the contract: tools pin it with literal-string tests.
//!
//! `data`/`error` must be plain data whose `Serialize` cannot fail (no
//! non-string map keys, no fallible custom impls) — every tool's `Payload`
//! and `AppError` are.

use serde::Serialize;

#[derive(Serialize)]
struct SuccessEnvelope<'a, T: ?Sized> {
    ok: bool,
    command: &'a str,
    data: &'a T,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a, E: ?Sized> {
    ok: bool,
    command: &'a str,
    error: &'a E,
}

pub fn success_json<T: Serialize + ?Sized>(command: &str, data: &T) -> String {
    let envelope = SuccessEnvelope {
        ok: true,
        command,
        data,
    };
    serde_json::to_string(&envelope)
        .expect("success envelope serialization is infallible for plain data")
}

pub fn error_json<E: Serialize + ?Sized>(command: &str, error: &E) -> String {
    let envelope = ErrorEnvelope {
        ok: false,
        command,
        error,
    };
    serde_json::to_string(&envelope)
        .expect("error envelope serialization is infallible for plain data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Data {
        id: &'static str,
        note: Option<&'static str>,
    }

    #[derive(Serialize)]
    struct Failure {
        kind: &'static str,
        message: &'static str,
    }

    #[test]
    fn success_envelope_orders_ok_command_data() {
        let data = Data {
            id: "x-1",
            note: None,
        };
        assert_eq!(
            success_json("decision show", &data),
            r#"{"ok":true,"command":"decision show","data":{"id":"x-1","note":null}}"#
        );
    }

    #[test]
    fn error_envelope_orders_ok_command_error() {
        let error = Failure {
            kind: "not_found",
            message: "no item x-1",
        };
        assert_eq!(
            error_json("show", &error),
            r#"{"ok":false,"command":"show","error":{"kind":"not_found","message":"no item x-1"}}"#
        );
    }
}
