use std::sync::Arc;

use aurora_domain::{SubverticalInmobiliaria, Ubicacion, VerticalComercial};
use aurora_osint::{
    clave_entidad_inegi, ConsultaObjetivo, DenueInegiAdapter, DestiladorOsint, EnjambreOsint,
    EntityResolver, HttpSwarmClient, OsintSource, PipelineOsint, PoliticaHttp,
    WebPublicaMxAdapter,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const FIXTURE_DENUE_MONTERREY_JSON: &str = r#"[
  {
    "CLEE": "19039236211000411000000000U5",
    "Id": "6198421",
    "Nombre": "PARQUE INDUSTRIAL APODACA LOGISTICA",
    "Razon_social": "DESARROLLOS INDUSTRIALES DEL NORTE S.A. DE C.V.",
    "Clase_actividad": "Edificación de naves y plantas industriales, excepto la supervisión",
    "Estrato": "101 a 250 personas",
    "Tipo_vialidad": "AVENIDA",
    "Calle": "CARLOS SALINAS DE GORTARI",
    "Num_Exterior": "450",
    "Colonia": "PARQUE INDUSTRIAL APODACA",
    "CP": "66600",
    "Ubicacion": "CIUDAD APODACA, APODACA, NUEVO LEÓN",
    "Telefono": "8183451122",
    "Correo_e": "contacto@desarrollosnorte.mx",
    "Sitio_internet": "www.desarrollosnorte.mx",
    "Latitud": "25.7811",
    "Longitud": "-100.1886"
  },
  {
    "CLEE": "19039484129000181000000000U2",
    "Id": "6198422",
    "Nombre": "TRANSPORTES REFRIGERADOS REGIOMONTANOS",
    "Razon_social": "TRANSPORTES REFRIGERADOS REGIOMONTANOS S.A. DE C.V.",
    "Clase_actividad": "Autotransporte foráneo con refrigeración",
    "Estrato": "51 a 100 personas",
    "Tipo_vialidad": "BOULEVARD",
    "Calle": "MIGUEL ALEMAN",
    "Num_Exterior": "1200",
    "Colonia": "VALLE SOLEADO",
    "CP": "67114",
    "Ubicacion": "GUADALUPE, GUADALUPE, NUEVO LEÓN",
    "Telefono": "8189004455",
    "Correo_e": "direccion@trregiomontanos.mx",
    "Sitio_internet": "https://trregiomontanos.mx",
    "Latitud": "25.7022",
    "Longitud": "-100.2145"
  }
]"#;

const FIXTURE_HTML_CORPORATIVO_MX: &str = r#"
<!DOCTYPE html>
<html lang="es-MX">
<body>
  <article class="nota-corporativa">
    <h2><a href="/negocios/expansion-industrial-apodaca-2026">Grupo Industrial Regio expande nave logística de 18,000 m² en Monterrey</a></h2>
    <span class="empresa">DESARROLLOS INDUSTRIALES DEL NORTE S.A. DE C.V.</span>
    <span class="directivo">Roberto Garza Cantú</span>
    <span class="co-mencion" data-contexto="Socia financiera en expansión industrial">Elena Morales Treviño</span>
    <p class="resumen">La compañía anunció expansión logística y compra de nave industrial con alta TIR proyectada en Apodaca, Nuevo León. Contacto público: contacto@desarrollosnorte.mx.</p>
  </article>

  <article class="nota-corporativa">
    <h2><a href="https://noticias.ejemplo.mx/veracruz/nombramiento-clinica">Clínica Médica del Golfo nombra a Fernando Castro Ríos como director general</a></h2>
    <p class="resumen">Clínica Médica del Golfo S.A. de C.V. inaugura nueva torre hospitalaria y contrata 120 especialistas en Boca del Río, Veracruz.</p>
  </article>

  <article class="nota-corporativa" data-opt-out="true">
    <h2><a href="/avisos/baja-arco">Aviso de oposición ARCO</a></h2>
    <span class="directivo">Luis Exclusión Pérez</span>
    <p class="resumen">Titular ejerció derecho de oposición ARCO y opt-out comercial.</p>
  </article>
</body>
</html>
"#;

fn consulta_monterrey_industrial() -> ConsultaObjetivo {
    ConsultaObjetivo {
        vertical: VerticalComercial::Inmobiliaria(SubverticalInmobiliaria::ComercialIndustrial),
        ubicacion: Ubicacion {
            ciudad: "Monterrey".to_string(),
            estado: "Nuevo León".to_string(),
            zona: Some("Apodaca".to_string()),
        },
        palabras_clave: vec!["nave industrial".to_string(), "logística".to_string()],
        limite_resultados: 10,
        requiere_js: false,
    }
}

#[tokio::test]
async fn denue_sin_token_se_omite_limpiamente_sin_error() -> Result<(), Box<dyn std::error::Error>> {
    let http = HttpSwarmClient::new(PoliticaHttp::default())?;
    let adapter = DenueInegiAdapter::new(None, http.clone());
    assert!(!adapter.tiene_token());

    let consulta = consulta_monterrey_industrial();
    let res = adapter.recolectar(&consulta).await?;
    assert!(res.is_empty());

    let adapter_vacio = DenueInegiAdapter::new(Some("   ".to_string()), http);
    assert!(!adapter_vacio.tiene_token());
    let res_vacio = adapter_vacio.recolectar(&consulta).await?;
    assert!(res_vacio.is_empty());

    assert_eq!(clave_entidad_inegi("Nuevo León"), "19");
    assert_eq!(clave_entidad_inegi("Veracruz"), "30");
    assert_eq!(clave_entidad_inegi("CDMX"), "09");
    assert_eq!(clave_entidad_inegi("Jalisco"), "14");
    Ok(())
}

#[tokio::test]
async fn pipeline_osint_mx_integra_denue_y_web_publica_deduplica_y_respeta_lfpdppp(
) -> Result<(), Box<dyn std::error::Error>> {
    let http = HttpSwarmClient::new(PoliticaHttp::default())?;
    let denue = DenueInegiAdapter::desde_fixture(FIXTURE_DENUE_MONTERREY_JSON, http.clone());
    let web_mx = WebPublicaMxAdapter::desde_fixture_html(
        "WebPublicaMX",
        "https://boletin-empresarial.ejemplo.mx/monterrey",
        FIXTURE_HTML_CORPORATIVO_MX,
        http,
    );

    let mut enjambre = EnjambreOsint::new(4);
    enjambre.registrar_fuente(Arc::new(denue));
    enjambre.registrar_fuente(Arc::new(web_mx));

    let pipeline = PipelineOsint::new(
        enjambre,
        DestiladorOsint::new(),
        EntityResolver::default(),
    );

    let consulta = consulta_monterrey_industrial();
    let ingesta = pipeline.ejecutar(&consulta).await?;

    // 2 registros DENUE + 3 notas HTML = 5 registros crudos totales
    assert_eq!(ingesta.registros_crudos_totales, 5);
    // El registro con data-opt-out="true" debe quedar excluido por cumplimiento LFPDPPP
    assert_eq!(ingesta.excluidos_opt_out.len(), 1);
    assert!(ingesta
        .prospectos
        .iter()
        .all(|p| !p.nombre_normalizado.to_lowercase().contains("exclus")));

    // Deduplicación por correo común (`contacto@desarrollosnorte.mx`) fusiona el registro DENUE
    // y la nota HTML de Roberto Garza Cantú, dejando 3 prospectos únicos válidos.
    assert_eq!(ingesta.prospectos.len(), 3);

    for p in &ingesta.prospectos {
        assert!(!p.lfpdppp.opt_out);
        assert!(
            p.lfpdppp.url_evidencia.starts_with("https://"),
            "URL verificable esperada, recibido: {}",
            p.lfpdppp.url_evidencia
        );
        assert!(
            p.lfpdppp.fuente_origen == "INEGI-DENUE" || p.lfpdppp.fuente_origen == "WebPublicaMX",
            "fuente inesperada: {}",
            p.lfpdppp.fuente_origen
        );
    }

    assert!(ingesta
        .prospectos
        .iter()
        .any(|p| p.lfpdppp.fuente_origen == "INEGI-DENUE"
            && p.lfpdppp.url_evidencia.contains("inegi.org.mx/app/mapa/denue")));

    Ok(())
}

#[tokio::test]
async fn denue_y_web_publica_consultan_servidor_http_local_con_rustls_y_user_agent(
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server = tokio::spawn(async move {
        let mut rutas = Vec::new();
        for _ in 0..2 {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0_u8; 2048];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let primera_linea = req.lines().next().unwrap_or("").to_string();
            rutas.push(primera_linea.clone());

            let (content_type, body) = if primera_linea.contains("/denue/") {
                ("application/json", FIXTURE_DENUE_MONTERREY_JSON)
            } else {
                ("text/html; charset=utf-8", FIXTURE_HTML_CORPORATIVO_MX)
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
        rutas
    });

    let http = HttpSwarmClient::new(PoliticaHttp::default())?;
    let denue = DenueInegiAdapter::con_api_base(
        Some("TEST-TOKEN-INEGI".to_string()),
        format!("http://{addr}/denue/BuscarEntidad"),
        http.clone(),
    );
    let mut consulta = consulta_monterrey_industrial();
    consulta.limite_resultados = 2;

    let regs_denue = denue.recolectar(&consulta).await?;
    assert_eq!(regs_denue.len(), 2);
    assert_eq!(regs_denue[0].fuente_origen, "INEGI-DENUE");

    let web = WebPublicaMxAdapter::con_urls(
        "WebPublicaMX",
        vec![format!("http://{addr}/mx/noticias")],
        http,
    );
    let regs_web = web.recolectar(&consulta).await?;
    assert_eq!(regs_web.len(), 2);

    let rutas = server.await?;
    assert!(rutas.iter().any(|r| r.contains("/denue/BuscarEntidad/") && r.contains("/19/1/2/TEST-TOKEN-INEGI")));
    assert!(rutas.iter().any(|r| r.contains("/mx/noticias")));
    Ok(())
}
