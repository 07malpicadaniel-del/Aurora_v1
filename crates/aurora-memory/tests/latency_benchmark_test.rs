use std::sync::Arc;
use std::time::Instant;

use aurora_domain::{
    CapacidadAdquisitiva, GatilloVital, MetadatosLfpdppp, PerfilPsicologico, Prospecto,
    RelacionTopologica, SubverticalInmobiliaria, TipoGatilloVital, TipoVinculo, Ubicacion,
    VerticalComercial,
};
use aurora_memory::{
    ConfigDecaimiento4D, ConsultaHibrida, DeterministicSimdEmbedder, IslaVertical, MemoryResult,
    MotorMemoriaAurora, MotorRecuperacion, TextEmbedder,
};
use chrono::{Duration, Utc};
use uuid::Uuid;

#[test]
fn benchmark_latencia_subgrafo_1000_nodos_3000_aristas_bajo_15ms() -> MemoryResult<()> {
    let ahora = Utc::now();
    let embedder = Arc::new(DeterministicSimdEmbedder::default_384());
    let mut motor = MotorMemoriaAurora::en_memoria(
        Arc::clone(&embedder) as Arc<dyn TextEmbedder>,
        ConfigDecaimiento4D::default(),
    );

    let isla = IslaVertical::from(VerticalComercial::Inmobiliaria(
        SubverticalInmobiliaria::ComercialIndustrial,
    ));

    let num_nodos = 1_000usize;
    let mut ids = Vec::with_capacity(num_nodos);
    let mut prospectos = Vec::with_capacity(num_nodos);

    for i in 0..num_nodos {
        let id = Uuid::new_v4();
        ids.push(id);
        let antiguedad_dias = (i % 90) as i64;
        prospectos.push(Prospecto {
            id,
            nombre_normalizado: format!("Director Industrial {i}"),
            aliases: vec![format!("Grupo Logistico {i}")],
            ubicacion: Ubicacion {
                ciudad: if i % 2 == 0 {
                    "Veracruz".to_string()
                } else {
                    "Queretaro".to_string()
                },
                estado: "Mexico".to_string(),
                zona: Some("Corredor Industrial".to_string()),
            },
            capacidad_adquisitiva: CapacidadAdquisitiva {
                ingreso_anual_estimado_mxn: 4_000_000 + (i as u64 * 10_000),
                patrimonio_estimado_mxn: Some(25_000_000),
            },
            perfil_psicologico: PerfilPsicologico::AnaliticoFinanciero,
            lfpdppp: MetadatosLfpdppp {
                fuente_origen: "DENUE Benchmark".to_string(),
                url_evidencia: format!("https://denue.ejemplo.mx/nodo/{i}"),
                capturado_en: ahora,
                opt_out: false,
            },
            gatillos: vec![GatilloVital {
                tipo: TipoGatilloVital::ExpansionLogistica,
                descripcion: format!("Apertura de nueva bodega aduanal y nave industrial #{i}"),
                ocurrido_en: ahora - Duration::days(antiguedad_dias),
                peso_urgencia: 0.75 + ((i % 20) as f32 * 0.01),
            }],
        });
    }

    // Construir 3,000 aristas deterministas (grado ~6 por nodo considerando simetría)
    let mut relaciones = Vec::with_capacity(3_000);
    for i in 0..num_nodos {
        for paso in [1usize, 7usize, 31usize] {
            let destino = (i + paso) % num_nodos;
            relaciones.push(RelacionTopologica {
                origen_id: ids[i],
                destino_id: ids[destino],
                tipo_vinculo: match paso {
                    1 => TipoVinculo::SocioComercial,
                    7 => TipoVinculo::MismoClubCirculo,
                    _ => TipoVinculo::InteraccionFrecuente,
                },
                fuerza: 0.85,
                contexto_compartido: format!("Cluster logistico {}", i / 10),
            });
        }
    }
    assert_eq!(relaciones.len(), 3_000);

    motor.ingestar_lote(prospectos, relaciones, isla)?;
    assert_eq!(motor.grafo().len_nodos(), 1_000);
    assert_eq!(motor.grafo().len_relaciones(), 3_000);

    let recuperador = MotorRecuperacion::default();
    let mut consulta = ConsultaHibrida::nueva(
        "nueva bodega aduanal nave industrial Veracruz",
        Some(isla),
    );
    consulta.ahora = ahora;
    consulta.top_k_broad = 32;
    consulta.top_k_final = 10;
    consulta.max_saltos_grafo = 2;

    // Warm-up de caché CPU/Rayon antes de medir latencia en régimen
    let _ = recuperador.consultar(&motor, &consulta)?;

    let t0 = Instant::now();
    let resultados = recuperador.consultar(&motor, &consulta)?;
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("Latencia consulta SubgrafoProspecto (1000 nodos, 3000 aristas): {elapsed_ms:.3} ms");

    assert_eq!(resultados.len(), 10);
    assert!(!resultados[0].vecinos_circulo_cercano.is_empty());
    assert!(
        elapsed_ms < 15.0,
        "Latencia de consulta excedió el presupuesto de 15 ms: {elapsed_ms:.3} ms"
    );

    Ok(())
}
