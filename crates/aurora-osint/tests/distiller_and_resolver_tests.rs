use std::sync::Arc;

use aurora_domain::{
    validar_prospecto, validar_relacion, PerfilPsicologico, SubverticalInmobiliaria,
    SubverticalSeguros, TipoGatilloVital, TipoVinculo, Ubicacion, VerticalComercial,
};
use aurora_osint::{
    clasificar_perfil_psicologico, detectar_gatillos, extraer_contactos, limpiar_ruido_html,
    DestiladorOsint, DirectorioPublicoAdapter, EnjambreOsint, EntityResolver, FormatoCrudo,
    HttpSwarmClient, OsintResult, PipelineOsint, PoliticaHttp, SenalesAbiertasAdapter,
    ConsultaObjetivo,
};
use chrono::Utc;
use std::collections::BTreeMap;

#[test]
fn destilador_limpia_html_extrae_contactos_y_detecta_gatillos_ambas_verticales() -> OsintResult<()>
{
    let html = r#"
        <html><head><style>.x{color:red}</style><script>alert(1)</script></head>
        <body>
            <h1>Nombramiento de nuevo director y expansión logística 2026</h1>
            <p>La empresa abrió nueva bodega aduanal en el puerto, buscando ROI y rentabilidad fiscal.
            Contacto: c.mendoza@logisticagolfo.mx | Tel: +52 229 555 0199 | @cmendoza_mx</p>
        </body></html>
    "#;

    let limpio = limpiar_ruido_html(html);
    assert!(!limpio.contains("alert(1)"));
    assert!(limpio.contains("nueva bodega aduanal"));

    let contactos = extraer_contactos(&limpio, &BTreeMap::new())?;
    assert!(contactos.correos.contains("c.mendoza@logisticagolfo.mx"));
    assert!(contactos.dominios.contains("logisticagolfo.mx"));
    assert!(contactos.handles_sociales.contains("@cmendoza_mx"));
    assert!(!contactos.telefonos.is_empty());

    let gatillos_inm = detectar_gatillos(&limpio, Utc::now());
    assert!(gatillos_inm
        .iter()
        .any(|g| g.tipo == TipoGatilloVital::ExpansionLogistica));
    assert!(gatillos_inm
        .iter()
        .any(|g| g.tipo == TipoGatilloVital::CambioPuesto));
    assert_eq!(
        clasificar_perfil_psicologico(&limpio, "DENUE"),
        PerfilPsicologico::AnaliticoFinanciero
    );

    let texto_seguros = "Constituyó nueva sociedad como socio fundador buscando protección de hombre clave, \
         legado familiar y fideicomiso educativo tras el nacimiento de su hijo.";
    let gatillos_seg = detectar_gatillos(texto_seguros, Utc::now());
    assert!(gatillos_seg
        .iter()
        .any(|g| g.tipo == TipoGatilloVital::AperturaEmpresa));
    assert!(gatillos_seg
        .iter()
        .any(|g| g.tipo == TipoGatilloVital::Nacimiento));
    assert_eq!(
        clasificar_perfil_psicologico(texto_seguros, "Registro Mercantil"),
        PerfilPsicologico::ConservadorPatrimonial
    );

    Ok(())
}

#[tokio::test]
async fn pipeline_deduplica_tres_variantes_infiere_socios_y_filtra_opt_out() -> OsintResult<()> {
    let fixture_dir_html = r#"
        <html><body>
          <article class="registro-empresa">
            <span class="razon-social">Logística del Golfo S.A. de C.V.</span>
            <span class="representante">Ing. Carlos Mendoza</span>
            <span class="socio">Lic. Sofía Navarro</span>
            <span class="giro">Almacenamiento y logística portuaria</span>
            <span class="ciudad">Veracruz</span>
            <span class="estado">Veracruz</span>
            <span class="contacto">contacto@logisticagolfo.mx | +52 229 888 1122</span>
            <a class="evidencia" href="https://directorio.ejemplo.mx/empresas/logistica-golfo">Ficha</a>
            <p>Inaugura nueva bodega aduanal en Veracruz con inversión en nave industrial.</p>
          </article>
          <article class="registro-empresa">
            <span class="razon-social">Logística del Golfo S.A. de C.V.</span>
            <span class="representante">Lic. Sofía Navarro</span>
            <span class="giro">Dirección financiera y patrimonio</span>
            <span class="ciudad">Veracruz</span>
            <span class="estado">Veracruz</span>
            <span class="contacto">snavarro@logisticagolfo.mx</span>
            <a class="evidencia" href="https://directorio.ejemplo.mx/empresas/sofia-navarro">Ficha</a>
            <p>Asume la dirección financiera para proteger el legado y patrimonio corporativo.</p>
          </article>
          <article class="registro-empresa" data-opt-out="true">
            <span class="razon-social">Privada Excluida S.A.</span>
            <span class="representante">Mariana López Ortiz</span>
            <span class="giro">Consultoría</span>
            <span class="ciudad">Veracruz</span>
            <span class="estado">Veracruz</span>
            <a class="evidencia" href="https://directorio.ejemplo.mx/empresas/excluida">Ficha</a>
          </article>
        </body></html>
    "#;

    let fixture_senales_json_1 = r#"[
        {
            "titulo": "Nombramiento en Logística del Golfo",
            "persona": "Carlos A. Mendoza",
            "empresa": "Logística del Golfo S.A. de C.V.",
            "co_menciones": [
                {
                    "nombre": "Sofía Navarro",
                    "contexto": "Socio comercial y directora financiera",
                    "empresa": "Logística del Golfo S.A. de C.V."
                }
            ],
            "contenido": "Carlos A. Mendoza fue nombrado director general tras el nacimiento de su hijo; prioriza ROI y deducibilidad fiscal.",
            "ciudad": "Veracruz",
            "estado": "Veracruz",
            "url_evidencia": "https://noticias.ejemplo.mx/carlos-mendoza-ascenso"
        }
    ]"#;

    let fixture_senales_html_2 = r#"
        <html><body>
          <article class="nota-corporativa">
            <h2 class="titulo">Expansión de terminal y nuevo hombre clave</h2>
            <span class="directivo">C. Mendoza - Logística del Golfo</span>
            <a class="fuente-link" href="https://camara.ejemplo.mx/evento-puerto">Nota Cámara</a>
            <p class="cuerpo">En el club empresarial del puerto anunció apertura de sociedad y nueva sucursal en 2026.</p>
          </article>
        </body></html>
    "#;

    let http = HttpSwarmClient::new(PoliticaHttp::default())?;
    let mut enjambre = EnjambreOsint::new(3);

    enjambre.registrar_fuente(Arc::new(DirectorioPublicoAdapter::desde_contenido(
        "Directorio Empresarial Publico",
        "https://directorio.ejemplo.mx/veracruz",
        FormatoCrudo::Html,
        fixture_dir_html,
        http.clone(),
    )));
    enjambre.registrar_fuente(Arc::new(SenalesAbiertasAdapter::desde_contenido(
        "Prensa Corporativa MX",
        "https://noticias.ejemplo.mx/feed",
        FormatoCrudo::Json,
        fixture_senales_json_1,
        http.clone(),
    )));
    enjambre.registrar_fuente(Arc::new(SenalesAbiertasAdapter::desde_contenido(
        "Boletin Camara Portuaria",
        "https://camara.ejemplo.mx/boletin",
        FormatoCrudo::Html,
        fixture_senales_html_2,
        http,
    )));

    let pipeline = PipelineOsint::new(enjambre, DestiladorOsint::new(), EntityResolver::default());

    let consulta = ConsultaObjetivo {
        vertical: VerticalComercial::Seguros(SubverticalSeguros::HombreClave),
        ubicacion: Ubicacion {
            ciudad: "Veracruz".to_string(),
            estado: "Veracruz".to_string(),
            zona: Some("Zona Portuaria".to_string()),
        },
        palabras_clave: vec!["logistica".to_string(), "hombre clave".to_string()],
        limite_resultados: 20,
        requiere_js: false,
    };

    let resultado = pipeline.ejecutar(&consulta).await?;

    // 5 registros crudos (3 directorio + 1 JSON + 1 HTML):
    // - 3 variantes de Carlos Mendoza se fusionan en 1 solo prospecto
    // - 1 prospecto Sofía Navarro
    // - 1 prospecto Mariana López con opt_out=true excluido por LFPDPPP
    assert_eq!(resultado.registros_crudos_totales, 5);
    assert_eq!(resultado.excluidos_opt_out.len(), 1);
    assert_eq!(resultado.prospectos.len(), 2);
    assert_eq!(resultado.payloads.len(), 2);

    let carlos = resultado
        .prospectos
        .iter()
        .find(|p| p.nombre_normalizado.contains("Carlos"))
        .ok_or_else(|| {
            aurora_osint::OsintError::ResolucionEntidades(
                "Carlos Mendoza no encontrado tras deduplicación".to_string(),
            )
        })?;

    assert!(
        carlos.aliases.iter().any(|a| a.contains("C. Mendoza"))
            || carlos.aliases.iter().any(|a| a.contains("Ing. Carlos Mendoza"))
            || carlos.aliases.iter().any(|a| a.contains("Carlos Mendoza"))
    );
    assert!(carlos.gatillos.len() >= 3);

    // Todos los prospectos finales deben pasar validación LFPDPPP de aurora-domain
    for p in &resultado.prospectos {
        validar_prospecto(p)?;
    }

    // Debe inferirse al menos 1 relación SocioComercial entre Carlos Mendoza y Sofía Navarro
    assert!(!resultado.relaciones.is_empty());
    assert!(resultado
        .relaciones
        .iter()
        .any(|r| r.tipo_vinculo == TipoVinculo::SocioComercial));
    for rel in &resultado.relaciones {
        validar_relacion(rel)?;
    }

    // Verifica que también funcione con consulta inmobiliaria
    assert_eq!(
        VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial).etiqueta(),
        "inmobiliaria/comercial_industrial"
    );

    Ok(())
}
