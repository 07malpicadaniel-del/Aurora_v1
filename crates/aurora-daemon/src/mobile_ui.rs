use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Página autocontenida del Portal Móvil de Revisión Rápida (Carril Ámbar sobre Tailscale).
pub const MOBILE_PORTAL_HTML: &str = include_str!("mobile_portal.html");

/// Sirve el portal móvil minimalista blanco y negro en `GET /mobile`.
pub async fn handler_mobile_portal() -> Html<&'static str> {
    Html(MOBILE_PORTAL_HTML)
}

#[derive(Debug, Serialize)]
struct UnauthorizedBody {
    error: &'static str,
    codigo: &'static str,
    detalle: &'static str,
}

/// Compara dos tokens en tiempo constante para evitar fugas por temporización.
#[must_use]
pub fn comparar_token_tiempo_constante(esperado: &str, recibido: &str) -> bool {
    let a = esperado.as_bytes();
    let b = recibido.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Valida la cabecera `Authorization: Bearer <token>` frente al secreto configurado.
#[must_use]
pub fn verificar_cabecera_bearer(headers: &HeaderMap, token_esperado: &str) -> bool {
    let Some(valor) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(texto) = valor.to_str() else {
        return false;
    };
    let Some(recibido) = texto.strip_prefix("Bearer ").map(str::trim) else {
        return false;
    };
    comparar_token_tiempo_constante(token_esperado.trim(), recibido)
}

/// Construye la respuesta HTTP `401 Unauthorized` estándar para accesos sin Bearer token válido.
#[must_use]
pub fn respuesta_no_autorizada() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"aurora-daemon\"")],
        Json(UnauthorizedBody {
            error: "UNAUTHORIZED",
            codigo: "UNAUTHORIZED",
            detalle: "Se requiere cabecera Authorization: Bearer <AURORA_MOBILE_TOKEN> válida",
        }),
    )
        .into_response()
}
