//! Prices from Renfe, venta.renfe.com, for trips in Spain.
//!
//! The site answers plain requests within a session. Opening its search link starts the
//! session, and the train list then comes from a DWR call, whose reply is a JavaScript object
//! literal. A search takes Renfe's station codes, such as 60000 for Madrid Puerta de Atocha, and
//! a date, and returns the trains of the day with the price of each fare.

use std::fmt::Write as _;

use reqwest::Client;
use serde_json::Value;

use crate::QuoteError;
use crate::site::{ConnectionSearch, ConnectionSource, SiteConnection, check_status, lowest};

/// The address of the search link.
const SEARCH_URL: &str = "https://venta.renfe.com/vol/buscarTren.do";

/// The address of the DWR calls.
const DWR_URL: &str = "https://venta.renfe.com/vol/dwr/call/plaincall";

/// The page that the DWR calls are made from.
const DWR_PAGE: &str = "%2Fvol%2FbuscarTrenEnlaces.do";

/// Renfe, read with plain requests.
#[derive(Debug, Default)]
pub struct Renfe;

/// Returns the link of a search on the site, which runs the search when opened. The site
/// shows the station names it is given, and searches by the codes.
fn search_link(
    origin_code: &str,
    origin_name: &str,
    destination_code: &str,
    destination_name: &str,
    date: chrono::NaiveDate,
) -> String {
    let encode = |text: &str| {
        text.bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => {
                    char::from(byte).to_string()
                }
                b' ' => "+".to_owned(),
                _ => format!("%{byte:02X}"),
            })
            .collect::<String>()
    };
    let date = encode(&date.format("%d/%m/%Y").to_string());
    format!(
        "{SEARCH_URL}?Idioma=en&Pais=UK&tipoBusqueda=autocomplete&currenLocation=menuBusqueda\
         &vengoderenfecom=SI&desOrigen={}&desDestino={}&cdgoOrigen=0071%2C{origin_code}%2C\
         {origin_code}&cdgoDestino=0071%2C{destination_code}%2C{destination_code}\
         &idiomaBusqueda=ES&FechaIdaSel={date}&_fechaIdaVisual={date}&adultos_=1&ninos_=0\
         &ninosMenores=0&plazaH=false&sinEnlace=false",
        encode(origin_name),
        encode(destination_name),
    )
}

/// Returns the reply value of a DWR answer, the literal after the call's two ids.
fn reply_value(answer: &str) -> Option<Value> {
    let call = answer.find("r.handleCallback(")?;
    let after_ids = answer[call..].splitn(3, ',').nth(2)?;
    crate::js_literal::parse(after_ids).map(|(value, _)| value)
}

/// Builds the body of the DWR call for the train list.
fn train_list_call(search: &ConnectionSearch, script_session_id: &str) -> String {
    let date = search
        .departure
        .format("%d/%m/%Y")
        .to_string()
        .replace('/', "%2F");
    let fields = [
        ("atendo", "false".to_owned()),
        ("sinEnlace", "false".to_owned()),
        ("plazaH", "false".to_owned()),
        ("tipoFranjaI", String::new()),
        ("tipoFranjaV", String::new()),
        ("horaFranjaIda", String::new()),
        ("horaFranjaVuelta", String::new()),
        ("fechaSalida", date.clone()),
        ("fechaVuelta", date),
        ("adultos", "1".to_owned()),
        ("ninos", "0".to_owned()),
        ("ninosMenores", "0".to_owned()),
        ("trayecto", "I".to_owned()),
        ("idaVuelta", String::new()),
        ("conMascota", "false".to_owned()),
        ("conBicicleta", "false".to_owned()),
        ("origen", search.origin.clone()),
        ("destino", search.destination.clone()),
        ("codPromo", String::new()),
    ];
    let mut body = String::from(
        "callCount=1\nwindowName=\nc0-scriptName=trainEnlacesManager\n\
         c0-methodName=getTrainsList\nc0-id=0\n",
    );
    let mut references = Vec::new();
    for (index, (name, value)) in fields.iter().enumerate() {
        let number = index + 1;
        let _ = writeln!(body, "c0-e{number}=string:{value}");
        references.push(format!("{name}:reference:c0-e{number}"));
    }
    let _ = write!(
        body,
        "c0-param0=Object_Object:{{{}}}\nbatchId=1\ninstanceId=0\npage={DWR_PAGE}\n\
         scriptSessionId={script_session_id}\n",
        references.join(", ")
    );
    body
}

/// Reads a price such as `37,35` with a decimal comma.
fn decimal_comma(text: &str) -> Option<f64> {
    text.replace(',', ".").parse().ok()
}

/// Reads the trains of the day and the lowest price of each class from the reply of the train
/// list. Class `T` is Turista, the second class, and `P` is Preferente, the first.
///
/// # Errors
///
/// Returns an error when the reply has no list of trains.
pub fn parse_connections(reply: &Value) -> Result<Vec<SiteConnection>, QuoteError> {
    let trips = reply
        .pointer("/listadoTrenes/0/listviajeViewEnlaceBean")
        .and_then(Value::as_array)
        .ok_or_else(|| QuoteError::Unexpected("no trains in the answer".to_owned()))?;
    Ok(trips
        .iter()
        .filter_map(|trip| {
            let date = trip.get("fecha")?.as_str()?;
            let time = trip.get("horaSalida")?.as_str()?.get(..5)?;
            let departure =
                chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M")
                    .ok()?;
            let fares = trip.get("tarifasDisponibles").and_then(Value::as_array);
            let trains = fares
                .and_then(|fares| fares.first())
                .and_then(|fare| fare.get("tarifasTramo")?.as_str())
                .map(|parts| {
                    parts
                        .split('#')
                        .filter_map(|part| part.split(',').next())
                        .map(|train| format!("Renfe {train}"))
                        .collect()
                })
                .unwrap_or_default();
            let class_price = |class: &str| {
                lowest(fares.into_iter().flatten().filter_map(|fare| {
                    (fare.get("cdgoClase").and_then(Value::as_str) == Some(class))
                        .then(|| fare.get("precioTarifa")?.as_str().and_then(decimal_comma))
                        .flatten()
                }))
            };
            let full = trip
                .get("completo")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let second_class_price = class_price("T");
            let first_class_price = class_price("P");
            Some(SiteConnection {
                departure,
                trains,
                not_for_sale: full || (second_class_price.is_none() && first_class_price.is_none()),
                second_class_price,
                first_class_price,
                currency: "EUR".to_owned(),
            })
        })
        .collect())
}

impl ConnectionSource for Renfe {
    const NAME: &'static str = "renfe";

    async fn fetch(
        &self,
        client: &Client,
        search: &ConnectionSearch,
    ) -> Result<Vec<SiteConnection>, QuoteError> {
        let link = search_link(
            &search.origin,
            "-",
            &search.destination,
            "-",
            search.departure.date(),
        );
        check_status(client.get(&link).send().await?, SEARCH_URL)?;
        let id_url = format!("{DWR_URL}/__System.generateId.dwr");
        let id_call = format!(
            "callCount=1\nc0-scriptName=__System\nc0-methodName=generateId\nc0-id=0\n\
             batchId=0\ninstanceId=0\npage={DWR_PAGE}\nscriptSessionId=\nwindowName=\n"
        );
        let response = client
            .post(&id_url)
            .header("content-type", "text/plain")
            .body(id_call)
            .send()
            .await?;
        let answer = check_status(response, &id_url)?.text().await?;
        let session = reply_value(&answer)
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| QuoteError::Unexpected("no DWR session id".to_owned()))?;
        // The page adds an id of its own after the session id, of this length.
        let script_session_id = format!("{session}/0000000-000000000");
        let list_url = format!("{DWR_URL}/trainEnlacesManager.getTrainsList.dwr");
        let response = client
            .post(&list_url)
            .header("content-type", "text/plain")
            .header("origin", "https://venta.renfe.com")
            .body(train_list_call(search, &script_session_id))
            .send()
            .await?;
        let answer = check_status(response, &list_url)?.text().await?;
        let reply = reply_value(&answer)
            .ok_or_else(|| QuoteError::Unexpected(answer.chars().take(200).collect::<String>()))?;
        parse_connections(&reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dwr_reply_gives_the_trains_with_their_lowest_fares() {
        let answer = r#"throw 'allowScriptTagRemoting is false.';
(function(){
var r=window.dwr._[0];
r.handleCallback("1","0",{listadoTrenes:[{listviajeViewEnlaceBean:[{fecha:"2026-11-20",
horaSalida:"06:27",completo:false,tarifasDisponibles:[
{cdgoClase:"T",precioTarifa:"41,1",tarifasTramo:"03301,N2010"},
{cdgoClase:"T",precioTarifa:"37,35",tarifasTramo:"03301,N1010"},
{cdgoClase:"P",precioTarifa:"44,7",tarifasTramo:"03301,N1010"}]}]}]});
})();"#;
        let connections = parse_connections(&reply_value(answer).unwrap()).unwrap();
        assert_eq!(connections[0].trains, ["Renfe 03301"]);
        assert_eq!(connections[0].second_class_price, Some(37.35));
        assert_eq!(connections[0].first_class_price, Some(44.7));
        assert!(!connections[0].not_for_sale);
    }

    #[test]
    fn the_search_link_carries_the_codes_and_the_date() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let link = search_link("60000", "Madrid Atocha", "71801", "Barcelona Sants", date);
        assert!(link.contains("desOrigen=Madrid+Atocha"), "{link}");
        assert!(link.contains("cdgoOrigen=0071%2C60000%2C60000"), "{link}");
        assert!(link.contains("FechaIdaSel=20%2F11%2F2026"), "{link}");
        assert!(!link.contains(char::is_whitespace), "{link}");
    }
}
