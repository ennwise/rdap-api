use icann_rdap_common::response::RdapResponse;
use serde_json::Value;
use tokio::{self, time::sleep, time::Duration};
use serde::{Deserialize, Serialize};
use icann_rdap_client::{
    create_client, rdap_bootstrapped_request, ClientConfig, MemoryBootstrapStore, QueryType,
    RdapClientError,
};
use std::str::FromStr;
use warp::Filter;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use chrono::{DateTime, Utc};

#[derive(Deserialize)]
struct ApiResponse {
    result: String,
}

#[derive(Deserialize, Serialize)]
struct QueryResponse {
    handle: String,
    registrant: Option<String>,
    administrative: Option<String>,
    fetched_at: String,
}

#[derive(Deserialize, Serialize)]
struct CachedRdapResponse {
    response: RdapResponse,
    fetched_at: String,
}
fn get_fn_value(arr: &Vec<Value>) -> Option<(String, bool)> {
    println!("Array: {:?}", arr);
    let mut fn_value = None;
    let mut is_org = false;

    for (i, item) in arr.iter().enumerate() {
        println!("Level 1 - item {}: {:?}", i, item);
        if let Value::Array(inner_array) = item {
            for (j, inner_item) in inner_array.iter().enumerate() {
                println!("  Level 2 - inner_item {}: {:?}", j, inner_item);
                if let Value::Array(deep_inner_array) = inner_item {
                    if deep_inner_array.len() == 4 {
                        if let (Value::String(key), Value::String(value)) =
                            (&deep_inner_array[0], &deep_inner_array[3])
                        {
                            println!("    Level 3 - key: {}, value: {}", key, value);
                            if key == "fn" {
                                fn_value = Some(value.clone());
                            } else if key == "kind" && value == "org" {
                                is_org = true;
                            }
                        }
                    }
                }
            }
        }
    }

    fn_value.map(|value| (value, is_org))
}

fn get_cache_dir() -> PathBuf {
    env::var("DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/data"))
}

fn read_cache(as_number: &str) -> io::Result<Option<CachedRdapResponse>> {
    let cache_dir = get_cache_dir();
    let cache_file = cache_dir.join(format!("{}.json", as_number));

    if cache_file.exists() {
        let mut file = File::open(cache_file)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let cached_response: CachedRdapResponse = serde_json::from_str(&contents)?;
        Ok(Some(cached_response))
    } else {
        Ok(None)
    }
}


fn write_cache(as_number: &str, response: &RdapResponse) -> io::Result<()> {
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;
    let cache_file = cache_dir.join(format!("{}.json", as_number));
    let mut file = File::create(cache_file)?;
    let cached_response = CachedRdapResponse {
        response: response.clone(),
        fetched_at: Utc::now().to_rfc3339(),
    };
    let contents = serde_json::to_string(&cached_response)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

async fn query_as_number(as_number: &str) -> Result<RdapResponse, RdapClientError> {
    let query = QueryType::from_str(as_number)?;
    let config = ClientConfig::default();
    let client = create_client(&config)?;
    let store = MemoryBootstrapStore::new();

    loop {
        let response = rdap_bootstrapped_request(&query, &client, &store, |reg| eprintln!("fetching {reg:?}")).await;

        match response {
            Ok(response) => {
                write_cache(as_number, &response.rdap)?;
                return Ok(response.rdap);
            }
            Err(RdapClientError::Client(ref e)) if e.status() == Some(warp::http::StatusCode::TOO_MANY_REQUESTS) => {
                eprintln!("Received 429 Too Many Requests, sleeping for 10 seconds...");
                sleep(Duration::from_secs(10)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

fn process_rdap_response(response: &RdapResponse) -> QueryResponse {
    let mut registrant_fn_value: Option<(String, bool)> = None;
    let mut administrative_fn_value: Option<(String, bool)> = None;
    let mut asnum = "".to_string();

    if let RdapResponse::Autnum(x) = response {
        let entities = x.object_common.entities.as_ref().unwrap();
        asnum = x.object_common.handle.clone().unwrap_or("N/A".to_string());

        for entity in entities {
            println!("Entity roles: {:?}", entity.roles);
            if let Some(roles) = &entity.roles {
                if roles.contains(&"registrant".to_string()) {
                    if let Some(arr) = &entity.vcard_array {
                        let fn_value = get_fn_value(arr);
                        if registrant_fn_value.is_none() || (registrant_fn_value.as_ref().unwrap().1 == false && fn_value.as_ref().map_or(false, |v| v.1)) {
                            registrant_fn_value = fn_value;
                        }
                    }
                }
                if roles.contains(&"administrative".to_string()) {
                    if let Some(arr) = &entity.vcard_array {
                        let fn_value = get_fn_value(arr);
                        if administrative_fn_value.is_none() || (administrative_fn_value.as_ref().unwrap().1 == false && fn_value.as_ref().map_or(false, |v| v.1)) {
                            administrative_fn_value = fn_value;
                        }
                    }
                }
            }
            // Break the loop if both values are found and both have .1 as true
            if registrant_fn_value.as_ref().map_or(false, |v| v.1) && administrative_fn_value.as_ref().map_or(false, |v| v.1) {
                break;
            }
        }
    }

    QueryResponse {
        handle: asnum,
        registrant: registrant_fn_value.map(|(value, _)| value),
        administrative: administrative_fn_value.map(|(value, _)| value),
        fetched_at: Utc::now().to_rfc3339(),
    }
}

#[tokio::main]
async fn main() {
    let asn_route = warp::path!("asn" / String)
        .and(warp::query::<std::collections::HashMap<String, String>>())
        .and_then(handle_asn_query);

    warp::serve(asn_route)
        .run(([127, 0, 0, 1], 3030))
        .await;
}

async fn handle_asn_query(asn: String, query_params: std::collections::HashMap<String, String>) -> Result<impl warp::Reply, warp::Rejection> {
    let no_cache = query_params.get("no_cache").map_or(false, |v| v == "true" || v == "1");

    if !no_cache {
        match read_cache(&asn) {
            Ok(Some(cached_response)) => {
                let mut query_response = process_rdap_response(&cached_response.response);
                query_response.fetched_at = cached_response.fetched_at;
                return Ok(warp::reply::json(&query_response));
            }
            Ok(None) => (), // No cached entry, proceed to query
            Err(e) => eprintln!("Error reading cache: {}", e), // Log error and proceed to query
        }
    }

    match query_as_number(&asn).await {
        Ok(rdap_response) => {
            let query_response = process_rdap_response(&rdap_response);
            Ok(warp::reply::json(&query_response))
        }
        Err(e) => Ok(warp::reply::json(&format!("Error: {}", e))),
    }
}