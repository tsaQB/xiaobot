//! JSON errors of the WebUI API: `{error, error_id, code, retry_after?}`.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    en: String,
    id: String,
    retry_after: Option<u64>,
}

pub(crate) type ApiResult<T> = Result<Json<T>, ApiError>;

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        en: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            en: en.into(),
            id: id.into(),
            retry_after: None,
        }
    }

    pub(crate) fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Sign in to continue.",
            "Masuk dulu untuk melanjutkan.",
        )
    }

    pub(crate) fn locked(retry_after: u64) -> Self {
        let minutes = retry_after.div_ceil(60).max(1);
        let mut error = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "locked",
            format!("Too many failed sign-ins. Try again in {minutes} min."),
            format!("Terlalu banyak percobaan gagal. Coba lagi dalam {minutes} menit."),
        );
        error.retry_after = Some(retry_after);
        error
    }

    pub(crate) fn bad_code() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "bad_code",
            "The code is wrong or has expired.",
            "Kode salah atau sudah kedaluwarsa.",
        )
    }

    pub(crate) fn bad_password() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "bad_password",
            "The password is wrong.",
            "Kata sandi salah.",
        )
    }

    pub(crate) fn not_configured(en: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "not_configured", en, id)
    }

    pub(crate) fn forbidden(en: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", en, id)
    }

    pub(crate) fn invalid(en: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid", en, id)
    }

    pub(crate) fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Not found.",
            "Tidak ditemukan.",
        )
    }

    pub(crate) fn conflict(en: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", en, id)
    }

    pub(crate) fn busy(en: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "busy", en, id)
    }

    /// A remote service failed; its message is passed on as-is in both languages.
    pub(crate) fn upstream(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self::new(StatusCode::BAD_GATEWAY, "upstream", detail.clone(), detail)
    }

    /// Something failed on the server. The detail goes to the log, not to the browser.
    pub(crate) fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::warn!("WebUI request failed: {detail}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "The server could not finish this request. See the log for details.",
            "Server gagal menyelesaikan permintaan ini. Lihat log untuk rinciannya.",
        )
    }

    /// Setting is overridden by the environment and cannot be changed here.
    pub(crate) fn env_locked(key: &str) -> Self {
        Self::conflict(
            format!("{key} is set in the environment, which always wins. Change it there."),
            format!("{key} diatur di environment, yang selalu menang. Ubah di sana."),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "error": self.en,
            "error_id": self.id,
            "code": self.code,
        });
        if let Some(retry_after) = self.retry_after {
            body["retry_after"] = json!(retry_after);
        }
        let mut response = (self.status, Json(body)).into_response();
        if let Some(retry_after) = self.retry_after {
            if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}
