use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use aurora_domain::{SubverticalInmobiliaria, VerticalComercial};
use aurora_factory::sha256_bytes;
use aurora_pulse::{
    ControladorTasaPlataforma, DeterministicMockSocialPublisher, GestorPublicacionOrganica,
    OrganicSocialPublisher, PlataformaPulso, PoliticaTasaPlataforma, PulseError,
    RecolectorTelemetria, SolicitudPublicacionOrganica,
};
use chrono::{Duration, Utc};
use uuid::Uuid;

fn crear_mp4_prueba(nombre: &str) -> Result<(PathBuf, String), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("aurora_social_pub_{}", Uuid::new_v4()));
    fs::create_dir_all(&dir)?;
    let ruta = dir.join(nombre);

    let mut mp4 = Vec::new();
    mp4.extend_from_slice(&24_u32.to_be_bytes());
    mp4.extend_from_slice(b"ftyp");
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(&512_u32.to_be_bytes());
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(b"mp42");
    fs::write(&ruta, &mp4)?;

    let hash = sha256_bytes(&mp4);
    Ok((ruta, hash))
}

#[tokio::test]
async fn publicador_organico_valida_sha256_y_bloquea_pii_en_carril_verde(
) -> Result<(), Box<dyn std::error::Error>> {
    let (ruta_mp4, sha256_ok) = crear_mp4_prueba("reel_educativo.mp4")?;
    let rate = Arc::new(ControladorTasaPlataforma::default());
    let mock_yt = Arc::new(DeterministicMockSocialPublisher::new(
        PlataformaPulso::YouTubeShorts,
    ));
    let gestor = GestorPublicacionOrganica::new(Arc::clone(&rate)).con_publicador(mock_yt);
    let telemetria = RecolectorTelemetria::new();

    let base = SolicitudPublicacionOrganica {
        campana_id: Uuid::new_v4(),
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
        titulo: "Por qué el nearshoring impulsa naves Clase A en Nuevo León".to_string(),
        descripcion: "Análisis educativo sobre plusvalía logística y retorno proyectado en parques industriales.".to_string(),
        hashtags: vec!["Nearshoring".to_string(), "InmobiliariaIndustrial".to_string()],
        ruta_mp4: ruta_mp4.clone(),
        sha256_mp4: sha256_ok.clone(),
        programado_para: None,
        libre_de_pii: true,
    };

    // 1. Bloqueo si libre_de_pii == false
    let mut sin_marca_pii = base.clone();
    sin_marca_pii.libre_de_pii = false;
    let err_bandera = gestor
        .publicar(PlataformaPulso::YouTubeShorts, &sin_marca_pii, &telemetria)
        .await;
    assert!(matches!(err_bandera, Err(PulseError::DespachoFallido { .. })));

    // 2. Bloqueo si contiene correo o teléfono mexicano aunque libre_de_pii == true
    let mut con_pii = base.clone();
    con_pii.descripcion =
        "Escríbenos a contacto@cliente-privado.mx o llama al +52 81 8345 6789".to_string();
    let err_pii = gestor
        .publicar(PlataformaPulso::YouTubeShorts, &con_pii, &telemetria)
        .await;
    assert!(matches!(err_pii, Err(PulseError::DespachoFallido { .. })));

    // 3. Bloqueo si el SHA-256 no coincide con el binario en NVMe
    let mut hash_roto = base.clone();
    hash_roto.sha256_mp4 = "0".repeat(64);
    let err_sha = gestor
        .publicar(PlataformaPulso::YouTubeShorts, &hash_roto, &telemetria)
        .await;
    assert!(matches!(err_sha, Err(PulseError::DespachoFallido { .. })));

    // 4. Publicación limpia pasa y registra evento en telemetría
    let comp = gestor
        .publicar(PlataformaPulso::YouTubeShorts, &base, &telemetria)
        .await?;
    assert_eq!(comp.plataforma, PlataformaPulso::YouTubeShorts);
    assert_eq!(comp.sha256_mp4, sha256_ok);
    assert!(comp.url_publica.starts_with("https://youtube.com/shorts/"));
    assert_eq!(telemetria.total_eventos(), 1);
    assert_eq!(gestor.historial().len(), 1);

    if let Some(dir) = ruta_mp4.parent() {
        let _ = fs::remove_dir_all(dir);
    }
    Ok(())
}

#[tokio::test]
async fn publicador_organico_respeta_rate_limit_y_abre_circuit_breaker_ante_429(
) -> Result<(), Box<dyn std::error::Error>> {
    let (ruta_mp4, sha256_ok) = crear_mp4_prueba("reel_meta.mp4")?;
    let rate = Arc::new(ControladorTasaPlataforma::default());
    rate.configurar_politica(
        PlataformaPulso::Instagram,
        PoliticaTasaPlataforma {
            cuota_max_hora: 10,
            cuota_max_dia: 20,
            intervalo_minimo_ms: 5_000,
            jitter_max_ms: 0,
            umbral_fallos_circuit_breaker: 2,
            enfriamiento_circuit_breaker_s: 600,
        },
    );

    let mock_ig = Arc::new(DeterministicMockSocialPublisher::new(
        PlataformaPulso::Instagram,
    ));
    let pub_ig = Arc::clone(&mock_ig) as Arc<dyn OrganicSocialPublisher>;
    let gestor = GestorPublicacionOrganica::new(Arc::clone(&rate)).con_publicador(pub_ig);
    let telemetria = RecolectorTelemetria::new();

    let sol = SolicitudPublicacionOrganica {
        campana_id: Uuid::new_v4(),
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::Preventa),
        titulo: "Tres claves fiscales antes de invertir en preventa vertical".to_string(),
        descripcion: "Guía educativa sobre plusvalía y planeación patrimonial sin asesoría individualizada.".to_string(),
        hashtags: vec!["PreventaMX".to_string(), "Patrimonio".to_string()],
        ruta_mp4: ruta_mp4.clone(),
        sha256_mp4: sha256_ok,
        programado_para: None,
        libre_de_pii: true,
    };

    let t0 = Utc::now();
    // Primera publicación ok
    let _ = gestor
        .publicar_en(PlataformaPulso::Instagram, &sol, &telemetria, t0)
        .await?;

    // Segunda publicación a +1 s viola intervalo mínimo (5 s) -> RateLimitPlataforma
    let err_intervalo = gestor
        .publicar_en(
            PlataformaPulso::Instagram,
            &sol,
            &telemetria,
            t0 + Duration::seconds(1),
        )
        .await;
    assert!(matches!(
        err_intervalo,
        Err(PulseError::RateLimitPlataforma { .. })
    ));

    // Inyectar 2 errores HTTP 429 consecutivos respetando intervalo -> abre circuit breaker
    mock_ig.inyectar_rate_limit_429(true);
    let err_429_1 = gestor
        .publicar_en(
            PlataformaPulso::Instagram,
            &sol,
            &telemetria,
            t0 + Duration::seconds(10),
        )
        .await;
    assert!(matches!(
        err_429_1,
        Err(PulseError::RateLimitPlataforma { .. })
    ));

    let err_429_2 = gestor
        .publicar_en(
            PlataformaPulso::Instagram,
            &sol,
            &telemetria,
            t0 + Duration::seconds(20),
        )
        .await;
    assert!(matches!(
        err_429_2,
        Err(PulseError::RateLimitPlataforma { .. })
    ));

    // Tercer intento (incluso si 429 se desactiva) es bloqueado por CircuitBreakerAbierto
    mock_ig.inyectar_rate_limit_429(false);
    let err_cb = gestor
        .publicar_en(
            PlataformaPulso::Instagram,
            &sol,
            &telemetria,
            t0 + Duration::seconds(30),
        )
        .await;
    assert!(matches!(
        err_cb,
        Err(PulseError::CircuitBreakerAbierto { .. })
    ));

    if let Some(dir) = ruta_mp4.parent() {
        let _ = fs::remove_dir_all(dir);
    }
    Ok(())
}
