use chrono::{TimeZone, Utc};
use uuid::Uuid;

use aurora_domain::{
    ActivoGenerado, AuroraError, AuroraResult, CampanaVertical, CapacidadAdquisitiva,
    EstadoAprobacion, GatilloVital, IntoVectorPayload, MetadatosLfpdppp, PerfilPsicologico,
    Prospecto, RelacionTopologica, SubverticalInmobiliaria, SubverticalSeguros,
    TipoActivoGenerado, TipoGatilloVital, TipoVinculo, Ubicacion, VectorPayload,
    VerticalComercial,
};

fn prospecto_muestra(opt_out: bool) -> Prospecto {
    let captura = Utc
        .with_ymd_and_hms(2026, 11, 14, 18, 0, 0)
        .earliest()
        .unwrap_or_else(Utc::now);
    let evento = Utc
        .with_ymd_and_hms(2026, 11, 10, 12, 30, 0)
        .earliest()
        .unwrap_or_else(Utc::now);

    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Carlos Eduardo Mendoza Ruiz".to_string(),
        aliases: vec!["Carlos Mendoza".to_string(), "Grupo Logistico Mendoza".to_string()],
        ubicacion: Ubicacion {
            ciudad: "Veracruz".to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Zona Portuaria".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 4_800_000,
            patrimonio_estimado_mxn: Some(22_500_000),
        },
        perfil_psicologico: PerfilPsicologico::AnaliticoFinanciero,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "INEGI DENUE Registro Publico".to_string(),
            url_evidencia: "https://www.inegi.org.mx/app/mapa/denue/".to_string(),
            capturado_en: captura,
            opt_out,
        },
        gatillos: vec![
            GatilloVital {
                tipo: TipoGatilloVital::ExpansionLogistica,
                descripcion: "Apertura de nueva bodega aduanal en el puerto".to_string(),
                ocurrido_en: evento,
                peso_urgencia: 0.88,
            },
            GatilloVital {
                tipo: TipoGatilloVital::Nacimiento,
                descripcion: "Anuncio publico de nacimiento de segundo hijo".to_string(),
                ocurrido_en: evento,
                peso_urgencia: 0.65,
            },
        ],
    }
}

#[test]
fn prospecto_json_y_binario_roundtrip() -> AuroraResult<()> {
    let original = prospecto_muestra(false);

    let json = original.to_json()?;
    let desde_json = Prospecto::from_json(&json)?;
    assert_eq!(original, desde_json);

    let bytes = original.to_bincode()?;
    let desde_binario = Prospecto::from_bincode(&bytes)?;
    assert_eq!(original, desde_binario);

    Ok(())
}

#[test]
fn entidades_comerciales_y_grafo_json_y_binario_roundtrip() -> AuroraResult<()> {
    let ahora = Utc::now();
    let campana = CampanaVertical {
        id: Uuid::new_v4(),
        nombre: "Naves Industriales Veracruz Q4".to_string(),
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
        objetivo_geografico: "Puerto de Veracruz".to_string(),
        creada_en: ahora,
        activa: true,
    };

    let relacion = RelacionTopologica {
        origen_id: Uuid::new_v4(),
        destino_id: Uuid::new_v4(),
        tipo_vinculo: TipoVinculo::SocioComercial,
        fuerza: 0.91,
        contexto_compartido: "Consejo de administracion en terminal portuaria".to_string(),
    };

    let activo = ActivoGenerado {
        id: Uuid::new_v4(),
        campana_id: campana.id,
        prospecto_id: relacion.origen_id,
        tipo: TipoActivoGenerado::DossierPdf,
        estado: EstadoAprobacion::PendienteGatekeeper,
        contenido_o_ruta: "/var/aurora/dossiers/dossier_01.pdf".to_string(),
        creado_en: ahora,
        actualizado_en: ahora,
        motivo_rechazo: None,
    };

    let campana_seguros = CampanaVertical {
        id: Uuid::new_v4(),
        nombre: "Blindaje Hombre Clave Bajio".to_string(),
        vertical: VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
        objetivo_geografico: "Leon / Queretaro".to_string(),
        creada_en: ahora,
        activa: true,
    };

    let campana_json = serde_json::to_string(&campana)?;
    let campana_rt: CampanaVertical = serde_json::from_str(&campana_json)?;
    assert_eq!(campana, campana_rt);
    assert_eq!(
        campana_seguros.vertical.etiqueta(),
        "seguros/hombre_clave"
    );

    let rel_bin =
        bincode::serialize(&relacion).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))?;
    let rel_rt: RelacionTopologica =
        bincode::deserialize(&rel_bin).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))?;
    assert_eq!(relacion, rel_rt);

    let activo_json = serde_json::to_string(&activo)?;
    let activo_rt: ActivoGenerado = serde_json::from_str(&activo_json)?;
    assert_eq!(activo, activo_rt);

    let activo_bin =
        bincode::serialize(&activo).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))?;
    let activo_bin_rt: ActivoGenerado =
        bincode::deserialize(&activo_bin).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))?;
    assert_eq!(activo, activo_bin_rt);

    Ok(())
}

#[test]
fn vector_payload_serializa_json_y_binario() -> AuroraResult<()> {
    let prospecto = prospecto_muestra(false);
    let payload = prospecto.to_vector_payload()?;

    assert_eq!(payload.prospecto_id, prospecto.id);
    assert_eq!(payload.gatillos_proyectados.len(), 2);
    assert!(payload.peso_inicial_4d >= 80);
    assert!(payload.texto_embedding.contains("Carlos Eduardo Mendoza Ruiz"));
    assert_eq!(
        payload.metadatos.get("lfpdppp_opt_out").map(String::as_str),
        Some("false")
    );

    let json = serde_json::to_string(&payload)?;
    let desde_json: VectorPayload = serde_json::from_str(&json)?;
    assert_eq!(payload, desde_json);

    let bin =
        bincode::serialize(&payload).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))?;
    let desde_bin: VectorPayload =
        bincode::deserialize(&bin).map_err(|e| AuroraError::SerializacionBinaria(e.to_string()))?;
    assert_eq!(payload, desde_bin);

    let payload_consumido = prospecto.into_vector_payload()?;
    assert_eq!(payload_consumido, payload);

    Ok(())
}
