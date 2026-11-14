use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    RelacionTopologica, SubverticalInmobiliaria, SubverticalSeguros, TipoGatilloVital, TipoVinculo,
    Ubicacion, VerticalComercial,
};
use aurora_memory::{
    ConfigDecaimiento4D, ConfigEmbedderMultilingue, ConsultaHibrida, DeterministicSimdEmbedder,
    IslaVertical, MemoryError, MemoryResult, ModeloMultilingue, MotorMemoriaAurora,
    MotorRecuperacion, MultilingualCpuEmbedder, TextEmbedder,
};
use aurora_osint::{
    ConsultaObjetivo, DestiladorOsint, DirectorioPublicoAdapter, EnjambreOsint, EntityResolver,
    FormatoCrudo, HttpSwarmClient, PipelineOsint, PoliticaHttp, SenalesAbiertasAdapter,
};
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefijo: &str) -> MemoryResult<Self> {
        let path = std::env::temp_dir().join(format!("{prefijo}_{}", Uuid::new_v4()));
        fs::create_dir_all(&path).map_err(|e| MemoryError::WalIo {
            ruta: path.clone(),
            source: e,
        })?;
        Ok(Self { path })
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn prospecto_base(
    nombre: &str,
    ciudad: &str,
    gatillos: Vec<GatilloVital>,
) -> Prospecto {
    Prospecto {
        id: Uuid::new_v4(),
        nombre_normalizado: nombre.to_string(),
        aliases: vec![],
        ubicacion: Ubicacion {
            ciudad: ciudad.to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Zona Portuaria".to_string()),
        },
        capacidad_adquisitiva: CapacidadAdquisitiva {
            ingreso_anual_estimado_mxn: 4_500_000,
            patrimonio_estimado_mxn: Some(20_000_000),
        },
        perfil_psicologico: PerfilPsicologico::AnaliticoFinanciero,
        lfpdppp: MetadatosLfpdppp {
            fuente_origen: "INEGI DENUE Publico".to_string(),
            url_evidencia: "https://www.inegi.org.mx/app/mapa/denue/".to_string(),
            capturado_en: Utc::now(),
            opt_out: false,
        },
        gatillos,
    }
}

fn gatillo(tipo: TipoGatilloVital, desc: &str, cuando: DateTime<Utc>, peso: f32) -> GatilloVital {
    GatilloVital {
        tipo,
        descripcion: desc.to_string(),
        ocurrido_en: cuando,
        peso_urgencia: peso,
    }
}

#[tokio::test]
async fn pipeline_osint_ingesta_en_aurora_memory_y_recupera_subgrafo() -> MemoryResult<()> {
    let fixture_html = r#"
        <html><body>
          <article class="registro-empresa">
            <span class="razon-social">Terminales del Golfo S.A.</span>
            <span class="representante">Ing. Carlos Mendoza</span>
            <span class="socio">Lic. Sofia Navarro</span>
            <span class="giro">Bodegas y logistica portuaria</span>
            <span class="ciudad">Veracruz</span>
            <span class="estado">Veracruz</span>
            <span class="contacto">cmendoza@terminalesgolfo.mx</span>
            <a class="evidencia" href="https://directorio.ejemplo.mx/terminales-golfo">Ficha</a>
            <p>Apertura de nueva bodega aduanal y nave industrial en 2026.</p>
          </article>
          <article class="registro-empresa">
            <span class="razon-social">Terminales del Golfo S.A.</span>
            <span class="representante">Lic. Sofia Navarro</span>
            <span class="giro">Finanzas y patrimonio</span>
            <span class="ciudad">Veracruz</span>
            <span class="estado">Veracruz</span>
            <span class="contacto">snavarro@terminalesgolfo.mx</span>
            <a class="evidencia" href="https://directorio.ejemplo.mx/sofia-navarro">Ficha</a>
            <p>Asume la direccion financiera de Terminales del Golfo.</p>
          </article>
        </body></html>
    "#;

    let fixture_json = r#"[{
        "titulo": "Expansión en el puerto",
        "persona": "Carlos A. Mendoza",
        "empresa": "Terminales del Golfo S.A.",
        "co_menciones": [{
            "nombre": "Sofia Navarro",
            "contexto": "Socio comercial y directora financiera",
            "empresa": "Terminales del Golfo S.A."
        }],
        "contenido": "Busca inversion en nave industrial con alto ROI.",
        "ciudad": "Veracruz",
        "estado": "Veracruz",
        "url_evidencia": "https://noticias.ejemplo.mx/terminales-golfo"
    }]"#;

    let http = HttpSwarmClient::new(PoliticaHttp::default())
        .map_err(|e| MemoryError::GrafoCausal(e.to_string()))?;
    let mut enjambre = EnjambreOsint::new(2);
    enjambre.registrar_fuente(Arc::new(DirectorioPublicoAdapter::desde_contenido(
        "DENUE Publico",
        "https://directorio.ejemplo.mx/veracruz",
        FormatoCrudo::Html,
        fixture_html,
        http.clone(),
    )));
    enjambre.registrar_fuente(Arc::new(SenalesAbiertasAdapter::desde_contenido(
        "Prensa Portuaria",
        "https://noticias.ejemplo.mx/feed",
        FormatoCrudo::Json,
        fixture_json,
        http,
    )));

    let pipeline = PipelineOsint::new(enjambre, DestiladorOsint::new(), EntityResolver::default());
    let vertical = VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial);
    let consulta_osint = ConsultaObjetivo {
        vertical,
        ubicacion: Ubicacion {
            ciudad: "Veracruz".to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Puerto".to_string()),
        },
        palabras_clave: vec!["bodega".to_string()],
        limite_resultados: 20,
        requiere_js: false,
    };

    let lote = pipeline
        .ejecutar(&consulta_osint)
        .await
        .map_err(|e| MemoryError::GrafoCausal(e.to_string()))?;
    assert_eq!(lote.prospectos.len(), 2);
    assert!(!lote.relaciones.is_empty());

    let embedder = Arc::new(DeterministicSimdEmbedder::default_384());
    let mut motor = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    let isla = IslaVertical::from(vertical);
    motor.ingestar_lote(lote.prospectos, lote.relaciones, isla)?;

    let recuperador = MotorRecuperacion::default();
    let consulta = ConsultaHibrida::nueva(
        "bodega aduanal nave industrial Veracruz ROI",
        Some(isla),
    );
    let subgrafos = recuperador.consultar(&motor, &consulta)?;

    assert_eq!(subgrafos.len(), 2);
    let top = &subgrafos[0];
    assert_eq!(top.isla, IslaVertical::InmobiliariaComercialIndustrial);
    assert!(!top.gatillos_vigentes.is_empty());
    assert!(!top.vecinos_circulo_cercano.is_empty());

    Ok(())
}

#[test]
fn decaimiento_4d_y_propagacion_circulo_cercano_1_y_2_saltos() -> MemoryResult<()> {
    let ahora = Utc::now();
    let hace_24h = ahora - Duration::hours(24);
    let hace_180d = ahora - Duration::days(180);

    let embedder = Arc::new(DeterministicSimdEmbedder::default_384());
    let mut motor = MotorMemoriaAurora::en_memoria(embedder, ConfigDecaimiento4D::default());
    let isla = IslaVertical::from(VerticalComercial::Seguros(SubverticalSeguros::HombreClave));

    let p_reciente = prospecto_base(
        "Director Logistico Reciente",
        "Monterrey",
        vec![gatillo(
            TipoGatilloVital::ExpansionLogistica,
            "Apertura de centro de distribucion logistica en Monterrey",
            hace_24h,
            0.90,
        )],
    );
    let p_antiguo = prospecto_base(
        "Director Logistico Antiguo",
        "Monterrey",
        vec![gatillo(
            TipoGatilloVital::ExpansionLogistica,
            "Apertura de centro de distribucion logistica en Monterrey",
            hace_180d,
            0.90,
        )],
    );

    let id_reciente = p_reciente.id;
    let id_antiguo = p_antiguo.id;
    motor.ingestar_prospecto(p_reciente, isla)?;
    motor.ingestar_prospecto(p_antiguo, isla)?;

    // Socios en cadena para verificar propagación de 1 y 2 saltos sin gatillo propio fuerte:
    // p_reciente --(SocioComercial)--> socio_1_salto --(MismoClubCirculo)--> socio_2_saltos
    // vs control_aislado (mismo texto/ciudad, sin aristas).
    let socio_1 = prospecto_base("Socio Directo Uno", "Monterrey", vec![]);
    let socio_2 = prospecto_base("Socio Indirecto Dos", "Monterrey", vec![]);
    let control_aislado = prospecto_base("Control Aislado Tres", "Monterrey", vec![]);

    let id_s1 = socio_1.id;
    let id_s2 = socio_2.id;
    let id_ctrl = control_aislado.id;

    motor.ingestar_prospecto(socio_1, isla)?;
    motor.ingestar_prospecto(socio_2, isla)?;
    motor.ingestar_prospecto(control_aislado, isla)?;

    motor.ingestar_relacion(RelacionTopologica {
        origen_id: id_reciente,
        destino_id: id_s1,
        tipo_vinculo: TipoVinculo::SocioComercial,
        fuerza: 0.92,
        contexto_compartido: "Socios fundadores en Grupo Norte".to_string(),
    })?;
    motor.ingestar_relacion(RelacionTopologica {
        origen_id: id_s1,
        destino_id: id_s2,
        tipo_vinculo: TipoVinculo::MismoClubCirculo,
        fuerza: 0.80,
        contexto_compartido: "Club Campestre Monterrey".to_string(),
    })?;

    let recuperador = MotorRecuperacion::default();
    let mut consulta = ConsultaHibrida::nueva("centro distribucion logistica Monterrey", Some(isla));
    consulta.ahora = ahora;
    consulta.top_k_final = 10;

    let res = recuperador.consultar(&motor, &consulta)?;
    let find_sg = |id: Uuid| res.iter().find(|s| s.prospecto.id == id);

    let sg_reciente = find_sg(id_reciente).ok_or(MemoryError::ProspectoNoEncontrado(id_reciente))?;
    let sg_antiguo = find_sg(id_antiguo).ok_or(MemoryError::ProspectoNoEncontrado(id_antiguo))?;
    let sg_s1 = find_sg(id_s1).ok_or(MemoryError::ProspectoNoEncontrado(id_s1))?;
    let sg_s2 = find_sg(id_s2).ok_or(MemoryError::ProspectoNoEncontrado(id_s2))?;
    let sg_ctrl = find_sg(id_ctrl).ok_or(MemoryError::ProspectoNoEncontrado(id_ctrl))?;

    // 1. Gatillo de 24h domina ampliamente al de 180 días
    assert!(sg_reciente.score_temporal_4d > sg_antiguo.score_temporal_4d * 10.0);
    assert!(sg_reciente.score_compuesto > sg_antiguo.score_compuesto);

    // 2. La señal se propaga a 1 salto (SocioComercial) y a 2 saltos sobre el control aislado
    assert!(sg_s1.boost_circulo_cercano > 0.25);
    assert!(sg_s2.boost_circulo_cercano > 0.05);
    assert_eq!(sg_ctrl.boost_circulo_cercano, 0.0);
    assert!(sg_s1.score_compuesto > sg_s2.score_compuesto);
    assert!(sg_s1.score_compuesto > sg_ctrl.score_compuesto);
    assert!(sg_s1.vecinos_circulo_cercano.iter().any(|v| v.salto == 1));
    assert!(sg_s2.vecinos_circulo_cercano.iter().any(|v| v.salto == 2));
    assert!(sg_reciente.vecinos_circulo_cercano.iter().any(|v| v.salto == 2));

    Ok(())
}

#[test]
fn persistencia_wal_y_purga_fisica_lfpdppp_en_nvme() -> MemoryResult<()> {
    let dir_guard = TempDirGuard::new("aurora_wal_lfpdppp")?;
    let ahora = Utc::now();
    let embedder = Arc::new(DeterministicSimdEmbedder::default_384());
    let isla = IslaVertical::SegurosPatrimonial;

    let p_purgar = prospecto_base(
        "Carlos Mendoza Purgado Secreto",
        "Veracruz",
        vec![gatillo(
            TipoGatilloVital::AperturaEmpresa,
            "Constituyo sociedad patrimonial exclusiva",
            ahora,
            0.85,
        )],
    );
    let p_conservar = prospecto_base(
        "Ana Patricia Conservada",
        "Veracruz",
        vec![gatillo(
            TipoGatilloVital::CompraActivo,
            "Adquirio residencia en zona costera",
            ahora,
            0.80,
        )],
    );

    let id_purgar = p_purgar.id;
    let id_conservar = p_conservar.id;

    // 1. Escribir en disco y cerrar instancia
    {
        let mut motor =
            MotorMemoriaAurora::abrir(&dir_guard.path, Arc::clone(&embedder) as Arc<dyn TextEmbedder>, ConfigDecaimiento4D::default())?;
        motor.ingestar_prospecto(p_purgar, isla)?;
        motor.ingestar_prospecto(p_conservar, isla)?;
        motor.ingestar_relacion(RelacionTopologica {
            origen_id: id_purgar,
            destino_id: id_conservar,
            tipo_vinculo: TipoVinculo::SocioComercial,
            fuerza: 0.9,
            contexto_compartido: "Inversion compartida".to_string(),
        })?;
        assert_eq!(motor.grafo().len_nodos(), 2);
        assert_eq!(motor.grafo().len_relaciones(), 1);
    }

    // 2. Reabrir desde WAL y verificar recuperación íntegra + purga LFPDPPP
    {
        let mut motor_recuperado =
            MotorMemoriaAurora::abrir(&dir_guard.path, Arc::clone(&embedder) as Arc<dyn TextEmbedder>, ConfigDecaimiento4D::default())?;
        assert_eq!(motor_recuperado.grafo().len_nodos(), 2);
        assert_eq!(motor_recuperado.grafo().len_relaciones(), 1);
        assert_eq!(motor_recuperado.islas().total_vectores(), 2);

        motor_recuperado.purgar_prospecto_opt_out(id_purgar)?;
        assert_eq!(motor_recuperado.grafo().len_nodos(), 1);
        assert_eq!(motor_recuperado.grafo().len_relaciones(), 0);
        assert_eq!(motor_recuperado.islas().total_vectores(), 1);
    }

    // 3. Reabrir por segunda vez y confirmar ausencia total en memoria, búsqueda y bytes en disco
    let motor_final =
        MotorMemoriaAurora::abrir(&dir_guard.path, embedder, ConfigDecaimiento4D::default())?;
    assert!(motor_final.esta_en_blacklist(id_purgar));
    assert!(motor_final.grafo().obtener_por_uuid(id_purgar).is_none());
    assert!(motor_final.grafo().obtener_por_uuid(id_conservar).is_some());
    assert_eq!(motor_final.grafo().len_relaciones(), 0);
    assert_eq!(motor_final.islas().total_vectores(), 1);

    let recuperador = MotorRecuperacion::default();
    let res = recuperador.consultar(
        &motor_final,
        &ConsultaHibrida::nueva("Carlos Mendoza Purgado Secreto", Some(isla)),
    )?;
    assert!(!res.iter().any(|s| s.prospecto.id == id_purgar));

    // Verificación física de borrado en disco (state.bin y wal.log sin rastro del nombre purgado)
    let state_bytes = fs::read(dir_guard.path.join("state.bin")).map_err(|e| MemoryError::WalIo {
        ruta: dir_guard.path.join("state.bin"),
        source: e,
    })?;
    let wal_bytes = fs::read(dir_guard.path.join("wal.log")).map_err(|e| MemoryError::WalIo {
        ruta: dir_guard.path.join("wal.log"),
        source: e,
    })?;
    let needle = b"Carlos Mendoza Purgado Secreto";
    assert!(!state_bytes.windows(needle.len()).any(|w| w == needle));
    assert!(!wal_bytes.windows(needle.len()).any(|w| w == needle));

    Ok(())
}

#[test]
fn multilingual_cpu_embedder_carga_diferida_y_prefijos() -> MemoryResult<()> {
    let dir_guard = TempDirGuard::new("aurora_embedder_model")?;

    let embedder_sin_fallback = MultilingualCpuEmbedder::new(ConfigEmbedderMultilingue {
        modelo: ModeloMultilingue::MultilingualE5Small,
        directorio_modelo: Some(dir_guard.path.clone()),
        num_hilos_cpu: 4,
        fallback_determinista_offline: false,
    })?;

    assert!(!embedder_sin_fallback.esta_cargado());
    assert_eq!(embedder_sin_fallback.dimension(), 384);
    assert_eq!(
        embedder_sin_fallback.formatear_texto("preventa residencial", true),
        "query: preventa residencial"
    );
    assert!(matches!(
        embedder_sin_fallback.embed("texto prueba"),
        Err(MemoryError::ModeloNoCargado(_))
    ));

    // Crear artefactos locales simulados sin descargar pesos de red
    fs::write(dir_guard.path.join("tokenizer.json"), br#"{"version":"1.0"}"#)
        .map_err(|e| MemoryError::WalIo {
            ruta: dir_guard.path.join("tokenizer.json"),
            source: e,
        })?;
    fs::write(dir_guard.path.join("model.onnx"), b"ONNX_DUMMY")
        .map_err(|e| MemoryError::WalIo {
            ruta: dir_guard.path.join("model.onnx"),
            source: e,
        })?;

    embedder_sin_fallback.cargar_desde_disco()?;
    assert!(embedder_sin_fallback.esta_cargado());
    let vec = embedder_sin_fallback.embed("nave industrial en el Bajio")?;
    assert_eq!(vec.len(), 384);

    Ok(())
}
