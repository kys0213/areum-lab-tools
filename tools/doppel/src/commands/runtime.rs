//! The async runtime for the commands that talk to (or are) the daemon. The
//! ledger commands stay synchronous; only these pay for a runtime.

use std::future::Future;

use crate::common::error::{AppError, ErrorKind};

pub(super) fn block_on<T>(
    future: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            AppError::new(
                ErrorKind::Internal,
                format!("cannot start async runtime: {e}"),
            )
        })?
        .block_on(future)
}
