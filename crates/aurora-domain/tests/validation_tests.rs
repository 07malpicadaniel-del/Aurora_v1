use chrono::Utc;
use uuid::Uuid;

use aurora_domain::{
    validar_activo, validar_campana, validar_prospecto, validar_relacion, ActivoGenerado,
    AuroraError, CampanaVertical, CapacidadAdquisitiva, EstadoAprobacion, GatilloVital,
    IntoVectorPayload, MetadatosLfpdppp, PerfilPsicologico, Prospecto, RelacionTopologica,
    SubverticalSeguros, TipoActivoGenerado, TipoGatilloVital, TipoVinculo, Ubicacion,
    VerticalComercial,
};

fn construir_prospecto_valido() -> Prospecto {
    let ahora = Utc::now();
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: "Ana Sofia Trevino Garza".to_string(),
        aliases: vec!["Ana Trevino".to_string()],
        ubicacion: Ubicacion {
            ciudad: "San Pedro Garza Garcia".to_string(),
            estado: "Nuevo Leon".to_string(),
            zona: Some("Valle Oriente".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 6_200_000,
            patrimonio_estimado_mxn: Some(35_000_000),
        },
        perfil_psicologico: PerfilPsicologico::ConservadorPatrimonial,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "Registro Publico de Comercio / Comunicado Corporativo".to_string(),
            url_evidencia: "https://rpc.economia.gob.mx/ejemplo-publico".to_string(),
            capturado_en: ahora,
            opt_out: false,
        },
        gatillos: vec![GatilloVital {
            tipo: TipoGatilloVital::CambioPuesto,
            descripcion: "Nombramiento como directora regional patrimonial".to_string(),
            ocurrido_en: ahora,
            peso_urgencia: 0.75,
        }],
    }
}

#[test]
fn prospecto_valido_pasa_validacion() {
    let prospecto = construir_prospecto_valido();
    assert!(validar_prospecto(&prospecto).is_ok());
}

#[test]
fn lfpdppp_bloquea_prospecto_con_opt_out_activo() {
    let mut prospecto = construir_prospecto_valido();
    let expected_id = prospecto.id;
    prospecto.lfpdppp.opt_out = true;

    let resultado = validar_prospecto(&prospecto);
    assert!(matches!(
        resultado,
        Err(AuroraError::OptOutActive { prospecto_id }) if prospecto_id == expected_id
    ));

    let payload_res = prospecto.to_vector_payload();
    assert!(matches!(
        payload_res,
        Err(AuroraError::OptOutActive { prospecto_id }) if prospecto_id == expected_id
    ));

    let payload_moved = prospecto.into_vector_payload();
    assert!(matches!(
        payload_moved,
        Err(AuroraError::OptOutActive { prospecto_id }) if prospecto_id == expected_id
    ));
}

#[test]
fn lfpdppp_bloquea_prospecto_sin_fuente_origen() {
    let mut prospecto = construir_prospecto_valido();
    prospecto.lfpdppp.fuente_origen = "   ".to_string();

    let resultado = validar_prospecto(&prospecto);
    assert!(matches!(
        resultado,
        Err(AuroraError::FuentePublicaFaltante { prospecto_id }) if prospecto_id == prospecto.id
    ));
}

#[test]
fn lfpdppp_bloquea_url_evidencia_invalida() {
    let mut prospecto = construir_prospecto_valido();
    prospecto.lfpdppp.url_evidencia = "ftp://archivo-privado.local/data.csv".to_string();

    let resultado = validar_prospecto(&prospecto);
    assert!(matches!(
        resultado,
        Err(AuroraError::UrlEvidenciaInvalida { prospecto_id, .. }) if prospecto_id == prospecto.id
    ));
}

#[test]
fn rechaza_pesos_fuera_de_rango_en_gatillo_y_relacion() {
    let mut prospecto = construir_prospecto_valido();
    prospecto.gatillos[0].peso_urgencia = 1.25;
    assert!(matches!(
        validar_prospecto(&prospecto),
        Err(AuroraError::PesoFueraDeRango {
            campo: "peso_urgencia",
            ..
        })
    ));

    prospecto.gatillos[0].peso_urgencia = f32::NAN;
    assert!(matches!(
        validar_prospecto(&prospecto),
        Err(AuroraError::PesoFueraDeRango {
            campo: "peso_urgencia",
            ..
        })
    ));

    let relacion_invalida = RelacionTopologica {
        origen_id: Uuid::new_v4(),
        destino_id: Uuid::new_v4(),
        tipo_vinculo: TipoVinculo::MismoClubCirculo,
        fuerza: -0.1,
        contexto_compartido: "Torneo anual de golf".to_string(),
    };
    assert!(matches!(
        validar_relacion(&relacion_invalida),
        Err(AuroraError::PesoFueraDeRango { campo: "fuerza", .. })
    ));
}

#[test]
fn rechaza_autorelacion_topologica() {
    let nodo = Uuid::new_v4();
    let relacion = RelacionTopologica {
        origen_id: nodo,
        destino_id: nodo,
        tipo_vinculo: TipoVinculo::Familiar,
        fuerza: 0.9,
        contexto_compartido: "Mismo apellido".to_string(),
    };

    assert!(matches!(
        validar_relacion(&relacion),
        Err(AuroraError::AutoRelacion { nodo_id }) if nodo_id == nodo
    ));
}

#[test]
fn valida_campana_y_regla_rechazo_gatekeeper() {
    let ahora = Utc::now();
    let campana = CampanaVertical {
        id: Uuid::new_v4(),
        nombre: "Patrimonial Monterrey".to_string(),
        vertical: VerticalComercial::Seguros(SubverticalSeguros::Patrimonial),
        objetivo_geografico: "Monterrey".to_string(),
        creada_en: ahora,
        activa: true,
    };
    assert!(validar_campana(&campana).is_ok());

    let activo_rechazado_sin_motivo = ActivoGenerado {
        id: Uuid::new_v4(),
        campana_id: campana.id,
        prospecto_id: Uuid::new_v4(),
        tipo: TipoActivoGenerado::ReelVerticalMp4,
        estado: EstadoAprobacion::Rechazado,
        contenido_o_ruta: "/tmp/reel.mp4".to_string(),
        creado_en: ahora,
        actualizado_en: ahora,
        motivo_rechazo: None,
    };
    assert!(matches!(
        validar_activo(&activo_rechazado_sin_motivo),
        Err(AuroraError::Validacion(_))
    ));
}
