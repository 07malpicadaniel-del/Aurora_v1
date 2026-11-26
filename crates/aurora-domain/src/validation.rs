use tracing::{debug, info_span, warn};
use uuid::Uuid;

use crate::error::{AuroraError, AuroraResult};
use crate::models::{
    ActivoGenerado, CampanaVertical, CatalogacionCompraBob, EstadoAprobacion, GatilloVital,
    NivelProbabilidadCompra, Prospecto, RelacionTopologica,
};

/// Verifica que un peso normalizado sea finito y esté dentro de `[0.0, 1.0]`.
pub fn validar_peso_unitario(campo: &'static str, valor: f32) -> AuroraResult<()> {
    if !valor.is_finite() || !(0.0..=1.0).contains(&valor) {
        return Err(AuroraError::PesoFueraDeRango { campo, valor });
    }
    Ok(())
}

/// Verifica que una URL de evidencia tenga esquema HTTP o HTTPS y contenido no vacío.
pub fn validar_url_publica(prospecto_id: Uuid, url: &str) -> AuroraResult<()> {
    let trimmed = url.trim();
    let es_http = trimmed.starts_with("https://") || trimmed.starts_with("http://");
    let tiene_host = trimmed
        .split_once("://")
        .is_some_and(|(_, rest)| !rest.trim().is_empty() && !rest.starts_with('/'));

    if !es_http || !tiene_host {
        return Err(AuroraError::UrlEvidenciaInvalida {
            prospecto_id,
            url: url.to_string(),
        });
    }
    Ok(())
}

/// Valida un `GatilloVital` individual antes de ingesta o proyección vectorial.
pub fn validar_gatillo(gatillo: &GatilloVital) -> AuroraResult<()> {
    if gatillo.descripcion.trim().is_empty() {
        return Err(AuroraError::Validacion(
            "la descripción del gatillo vital no puede estar vacía".to_string(),
        ));
    }
    validar_peso_unitario("peso_urgencia", gatillo.peso_urgencia)?;
    Ok(())
}

/// Valida de forma determinista un `Prospecto` antes de su ingesta en memoria o OSINT.
///
/// Reglas aplicadas:
/// - Bloqueo inmediato si `lfpdppp.opt_out == true`.
/// - Exige `fuente_origen` no vacía y `url_evidencia` HTTP/HTTPS verificable.
/// - Exige `nombre_normalizado`, `ubicacion.ciudad` y `ubicacion.estado` no vacíos.
/// - Valida cada `GatilloVital` y su `peso_urgencia` en `[0.0, 1.0]`.
pub fn validar_prospecto(prospecto: &Prospecto) -> AuroraResult<()> {
    let _span = info_span!("validar_prospecto", prospecto_id = %prospecto.id).entered();

    if prospecto.lfpdppp.opt_out {
        warn!(prospecto_id = %prospecto.id, "Prospecto bloqueado por bandera LFPDPPP opt_out");
        return Err(AuroraError::OptOutActive {
            prospecto_id: prospecto.id,
        });
    }

    if prospecto.lfpdppp.fuente_origen.trim().is_empty() {
        warn!(prospecto_id = %prospecto.id, "Prospecto rechazado por ausencia de fuente pública");
        return Err(AuroraError::FuentePublicaFaltante {
            prospecto_id: prospecto.id,
        });
    }

    validar_url_publica(prospecto.id, &prospecto.lfpdppp.url_evidencia)?;

    if prospecto.nombre_normalizado.trim().is_empty() {
        return Err(AuroraError::Validacion(format!(
            "el prospecto {} tiene nombre_normalizado vacío",
            prospecto.id
        )));
    }

    if prospecto.ubicacion.ciudad.trim().is_empty() || prospecto.ubicacion.estado.trim().is_empty()
    {
        return Err(AuroraError::Validacion(format!(
            "el prospecto {} debe especificar ciudad y estado",
            prospecto.id
        )));
    }

    for gatillo in &prospecto.gatillos {
        validar_gatillo(gatillo)?;
    }

    debug!(
        prospecto_id = %prospecto.id,
        gatillos = prospecto.gatillos.len(),
        "Prospecto validado correctamente bajo reglas de dominio y LFPDPPP"
    );
    Ok(())
}

/// Valida una arista topológica del Círculo Cercano antes de insertarla en el grafo.
pub fn validar_relacion(relacion: &RelacionTopologica) -> AuroraResult<()> {
    let _span = info_span!(
        "validar_relacion",
        origen = %relacion.origen_id,
        destino = %relacion.destino_id
    )
    .entered();

    if relacion.origen_id == relacion.destino_id {
        return Err(AuroraError::AutoRelacion {
            nodo_id: relacion.origen_id,
        });
    }

    validar_peso_unitario("fuerza", relacion.fuerza)?;

    if relacion.contexto_compartido.trim().is_empty() {
        return Err(AuroraError::Validacion(
            "contexto_compartido de RelacionTopologica no puede estar vacío".to_string(),
        ));
    }

    debug!("Relación topológica validada");
    Ok(())
}

/// Valida la definición de una campaña comercial.
pub fn validar_campana(campana: &CampanaVertical) -> AuroraResult<()> {
    if campana.nombre.trim().is_empty() {
        return Err(AuroraError::Validacion(format!(
            "la campaña {} debe tener un nombre no vacío",
            campana.id
        )));
    }
    if campana.objetivo_geografico.trim().is_empty() {
        return Err(AuroraError::Validacion(format!(
            "la campaña {} debe especificar objetivo_geografico",
            campana.id
        )));
    }
    Ok(())
}

/// Valida consistencia y estado de Gatekeeper de un `ActivoGenerado`.
pub fn validar_activo(activo: &ActivoGenerado) -> AuroraResult<()> {
    if activo.contenido_o_ruta.trim().is_empty() {
        return Err(AuroraError::Validacion(format!(
            "el activo {} requiere contenido_o_ruta no vacío",
            activo.id
        )));
    }
    if activo.estado == EstadoAprobacion::Rechazado
        && activo
            .motivo_rechazo
            .as_deref()
            .is_none_or(|m| m.trim().is_empty())
    {
        return Err(AuroraError::Validacion(format!(
            "el activo {} en estado Rechazado requiere motivo_rechazo",
            activo.id
        )));
    }
    Ok(())
}

/// Valida integridad de una calificación de probabilidad de compra emitida por BOB o heurística 4D.
pub fn validar_catalogacion_compra(cat: &CatalogacionCompraBob) -> AuroraResult<()> {
    validar_peso_unitario("probabilidad_compra", cat.probabilidad_compra)?;
    if !cat.score_topologico_4d.is_finite() || cat.score_topologico_4d < 0.0 {
        return Err(AuroraError::PesoFueraDeRango {
            campo: "score_topologico_4d",
            valor: cat.score_topologico_4d,
        });
    }
    let nivel_esperado = NivelProbabilidadCompra::desde_probabilidad(cat.probabilidad_compra);
    if cat.nivel != nivel_esperado {
        return Err(AuroraError::Validacion(format!(
            "nivel de probabilidad inconsistente ({:?} vs {:?}) para probabilidad {:.3}",
            cat.nivel, nivel_esperado, cat.probabilidad_compra
        )));
    }
    if cat.producto_recomendado.trim().is_empty() {
        return Err(AuroraError::Validacion(format!(
            "la catalogación de {} requiere producto_recomendado no vacío",
            cat.prospecto_id
        )));
    }
    if cat.justificacion_corta.trim().is_empty() {
        return Err(AuroraError::Validacion(format!(
            "la catalogación de {} requiere justificacion_corta no vacía",
            cat.prospecto_id
        )));
    }
    Ok(())
}
