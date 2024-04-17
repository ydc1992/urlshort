use std::cmp::Ordering;
use std::env;
use std::error::Error;
use std::path::PathBuf;

use axum::{extract::Path, Json, response::Redirect, Router, routing::get};
use axum::response::IntoResponse;
use axum::routing::post;
use elasticsearch::{CountParts, Elasticsearch, GetParts, http::transport::Transport, SearchParts};
use serde::{Deserialize, Serialize};
use serde_json::{json, Number, Value};
use sevenz_rust::*;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

fn read_7z_file(path: &str, name: &str, buff: &mut Vec<u8>) {
    decompress_file_with_extract_fn(
        path,
        "",
        |entry, reader, _dest| {
            let p = entry.to_owned();
            if p.name.cmp(&name.to_string()) == Ordering::Equal {
                let f = reader.read_to_end(buff);
                match f {
                    Err(r) => {
                        match r.kind() {
                            ChecksumVerificationFailed => {
                                //忽略晓校验和验证失败
                            }
                        }
                    }
                    _ => {}
                };
            }
            Ok(true)
        }).expect("TODO: panic message");
}


#[derive(Serialize, Deserialize, Debug)]
struct IMS {
    #[serde(skip_serializing_if = "Option::is_none")]
    zip: Option<bool>,
    format: String,
    articles_id: String,
    url: String,
    local: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct ART {
    img_url_in_db: bool,
    link: String,
    subcat: String,
    source: String,
    title: String,
    body: String,
    published_from: String,
}

fn decode<T: AsRef<[u8]>>(input: T) -> Vec<u8> {
    let inputstr = std::str::from_utf8(input.as_ref()).unwrap();
    base64::decode_config(inputstr, base64::URL_SAFE_NO_PAD).unwrap()
}


fn elk_init() -> Elasticsearch {
    let transport = Transport::single_node("http://192.168.0.101:9200").unwrap();
    Elasticsearch::new(transport)
}

async fn query_by_id(index: &str, id: &str) -> Result<Value, elasticsearch::Error> {
    let client = elk_init();

    let response = client
        .get(GetParts::IndexId(index, id))
        .send().await?;
    response.json::<Value>().await
}

async fn query_by_cond(_index: &str, cond: Value) -> Result<Value, elasticsearch::Error> {
    let client = elk_init();

    let response = client
        .search(SearchParts::None)
        .body(cond)
        .allow_no_indices(true)
        .send().await?;
    response.json::<Value>().await
}


#[derive(Serialize, Deserialize, Debug)]
struct User {
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_ix: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subcat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<String>,
}


async fn home1(Json(user): Json<User>) -> impl IntoResponse {
    if user.action.cmp(&"get_root_node".to_string()) == Ordering::Equal {
        let body = json!({
            "size":0,
            "aggs":{
                "popular_colors":{
                    "terms":{
                        "field": "source",
                        "size": 20000
                    }
                }
            }
        });
        let value = query_by_cond("articles".as_ref(), body).await.expect("query faild");
        let buckets = &value["aggregations"]["popular_colors"]["buckets"];


        let mut v1 = json!({});
        let articles_map = v1.as_object_mut().unwrap();

        if let Value::Array(ref array) = buckets {
            let mut n = 0;
            for item in array.iter() {
                let key = format!("{}", n);
                n = n + 1;
                articles_map.insert(key, item.to_owned());
            }
            v1.to_string()
        } else {
            "".to_string()
        }
    } else if user.action.cmp(&"get_subcat_node".to_string()) == Ordering::Equal {
        let source = user.source.unwrap();
        let body = json!({
            "size": 0,
            "query":{
                "term": {"source": source}
            },
            "aggs":{
                "popular_colors":{
                    "terms":{
                        "field": "subcat",
                        "size": 20000
                    }
                }
            }
        });

        let value = query_by_cond("articles".as_ref(), body).await.expect("query faild");
        let buckets = &value["aggregations"]["popular_colors"]["buckets"];

        let mut v1 = json!({});
        let articles_map = v1.as_object_mut().unwrap();

        if let Value::Array(ref array) = buckets {
            let mut n = 0;
            for item in array.iter() {
                let key = format!("{}", n);
                n = n + 1;
                articles_map.insert(key, item.to_owned());
            }
            v1.to_string()
        } else {
            "".to_string()
        }
    } else if user.action.cmp(&"get_cnt".to_string()) == Ordering::Equal {
        let source = user.source.unwrap();
        let body = json!({
            "query":{
                "term":{"source":source}
            }
        });
        let client = elk_init();
        let response = client
            .count(CountParts::None)
            .body(body)
            .allow_no_indices(true)
            .send().await.unwrap();
        let value = response.json::<Value>().await.expect("query faild");
        let mut v1 = json!({});
        let articles_map = v1.as_object_mut().unwrap();
        articles_map.insert("cnt".to_string(), value["count"].to_owned());
        v1.to_string()
    } else if user.action.cmp(&"get_item".to_string()) == Ordering::Equal {
        let source = user.source.unwrap();
        let page_ix = user.page_ix.unwrap().as_u64().unwrap();
        let subcat = user.subcat.unwrap();

        let mut body: Value;

        body = json!({
            "size":0x20,
            "from":page_ix*0x20,
            "_source":{
                 "include":["title","link","published_from","subcat"]
            },
            "sort":{
                "published_from":{ "order": "desc"}
            }
        });

        let p = body.as_object_mut().unwrap();
        if subcat.is_empty() {
            p.insert("query".to_string(), json!({
                 "bool":{
                    "filter":{"term": {"source": source}}
                }
            }));
        } else {
            p.insert("query".to_string(), json!({
                 "bool":{
                        "filter":{"term": {"source": source}},
                        "must":{"term": {"subcat": subcat}}
                }
            }));
        }

        let value = query_by_cond("articles".as_ref(), body).await.expect("query faild");
        let buckets = &value["hits"]["hits"];


        let mut v1 = json!({});
        let articles_map = v1.as_object_mut().unwrap();

        if let Value::Array(ref array) = buckets {
            let mut n = 0;
            for item in array.iter() {
                let key = format!("{}", n);
                n = n + 1;
                articles_map.insert(key, item["_source"].to_owned());
            }
            v1.to_string()
        } else {
            "".to_string()
        }
    } else if user.action.cmp(&"get_body".to_string()) == Ordering::Equal {
        let link = user.link.unwrap();

        let body = json!({
            "query":{
              "bool":{
                    "filter":{"term":{"link":link}}
                }
            }
        });

        let value = query_by_cond("articles".as_ref(), body).await.expect("query faild");
        let buckets = &value["hits"]["hits"];

        let mut v1 = json!({});
        let articles_map = v1.as_object_mut().unwrap();

        if let Value::Array(ref array) = buckets {
            for item in array.iter() {
                articles_map.insert("body".to_string(), item["_source"]["body"].to_owned());
            }
            v1.to_string()
        } else {
            "".to_string()
        }
    } else {
        "".to_string()
    }
}


fn get_base_dir() -> std::io::Result<PathBuf> {
    if cfg!(target_os = "linux") {
        env::current_dir()
    } else if cfg!(target_os = "windows") {
        Ok(PathBuf::from(r"\\NAS32FE5D\Public\urlshort\static"))
    } else {
        panic!("error platorm")
    }
}

fn get_js_path(name: &str) -> String {
    let css_path;
    let current_dir = get_base_dir();
    if cfg!(target_os = "linux") {
        css_path = format!("{}/static/js/{}",
                           current_dir.unwrap().to_str().unwrap().to_string(), name);
    } else if cfg!(target_os = "windows") {
        css_path = format!("{}\\js\\{}",
                           current_dir.unwrap().to_str().unwrap().to_string(), name);
    } else {
        panic!("error platorm");
    }
    css_path
}

fn get_image_7z_path(articles_id: &str, _name: &str, _images: &IMS) -> String {
    let current_dir = get_base_dir();
    let key = &articles_id.as_bytes()[0..3];
    let root_dir;
    let imag_path;
    if cfg!(target_os = "linux") {
        root_dir = format!("{}/static/img/{}",
                           current_dir.unwrap().to_str().unwrap().to_string(),
                           std::str::from_utf8(&key).unwrap());
        imag_path = format!("{root_dir}/{}.7z", articles_id);
    } else if cfg!(target_os = "windows") {
        root_dir = format!("{}\\img\\{}",
                           current_dir.unwrap().to_str().unwrap().to_string(),
                           std::str::from_utf8(&key).unwrap());
        imag_path = format!("{root_dir}\\{}.7z", articles_id);
    } else {
        panic!("error platorm");
    }
    imag_path
}

fn get_image_path(articles_id: &str, name: &str, images: &IMS) -> String {
    let current_dir = get_base_dir().unwrap().to_str().unwrap().to_string();
    let key = &articles_id.as_bytes()[0..3];
    let root_dir;
    let imag_path;
    if cfg!(target_os = "linux") {
        root_dir = format!("{}/static/img/{}/{}",
                           current_dir,
                           std::str::from_utf8(&key).unwrap(),
                           articles_id);
        imag_path = format!("{root_dir}/{}.{}", name, images.format);
    } else if cfg!(target_os = "windows") {
        root_dir = format!("{}\\img\\{}\\{}",
                           current_dir,
                           std::str::from_utf8(&key).unwrap(),
                           articles_id,
        );
        imag_path = format!("{root_dir}\\{}.{}", name, images.format);
    } else {
        panic!("error platorm");
    }
    imag_path
}

fn get_css_path(name: &str) -> String {
    let css_path;
    let current_dir = get_base_dir();
    if cfg!(target_os = "linux") {
        css_path = format!("{}/static/css/{}.css",
                           current_dir.unwrap().to_str().unwrap().to_string(), name);
    } else if cfg!(target_os = "windows") {
        css_path = format!("{}\\css\\{}.css",
                           current_dir.unwrap().to_str().unwrap().to_string(), name);
    } else {
        panic!("error platorm");
    }
    css_path
}

async fn read_css_file(Path(css_name): Path<String>) -> impl IntoResponse {
    let v = query_by_id("articles".as_ref(), &css_name).await.unwrap();
    let arts: ART = serde_json::from_value(v["_source"].to_owned()).unwrap();

    let css_path = get_css_path(arts.source.as_ref());

    let mut file = match File::open(&css_path).await {
        Ok(file) => file,
        Err(_err) => { panic!("{}", css_path) }
    };


    let content_type = match mime_guess::from_path(&css_path).first_raw() {
        Some(mime) => mime,
        None => { panic!("1234") }
    };

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await.expect("read file faild");

    (
        axum::response::AppendHeaders([
            (http::header::CONTENT_TYPE, content_type),
        ]),
        buffer
    ).into_response()
}


async fn read_js_file(Path(js_name): Path<String>) -> impl IntoResponse {
    let css_path = get_js_path(js_name.as_ref());
    let mut file = match File::open(&css_path).await {
        Ok(file) => file,
        Err(_err) => { panic!("{}", css_path) }
    };


    let content_type = match mime_guess::from_path(&css_path).first_raw() {
        Some(mime) => mime,
        None => { panic!("1234") }
    };

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await.expect("read file faild");
    (
        axum::response::AppendHeaders([
            (http::header::CONTENT_TYPE, content_type),
        ]),
        buffer
    ).into_response()
}

async fn check_image_exits(Path(name): Path<String>) -> impl IntoResponse {
    let img = query_by_id("articles_img".as_ref(), &name).await.unwrap();
    let imags: IMS = serde_json::from_value(img["_source"].to_owned()).unwrap();
    if imags.local == false && imags.format == "" {
        "false"
    } else {
        "true"
    }
}


#[derive(Serialize, Deserialize, Debug)]
struct UploadData {
    id: String,
    data: String,
}

// 重定向本地图片或者远程图片
async fn redirect_short_url(Path(name): Path<String>) -> impl IntoResponse {
    let img = query_by_id("articles_img".as_ref(), &name).await.unwrap();
    let imags: IMS;

    match serde_json::from_value(img["_source"].to_owned()) {
        Ok(img) => imags = img,
        _ => { return "".into_response(); }
    }

    if imags.local == false && imags.format == "" {
        let e = decode(imags.url);
        let y = std::str::from_utf8(&e).unwrap();
        Redirect::to(y).into_response()
    } else {
        let imag_7z_path = get_image_7z_path(imags.articles_id.as_ref(), &name, &imags);
        let target_path = std::path::Path::new(&imag_7z_path);
        if !target_path.exists() {
            let image_path = get_image_path(imags.articles_id.as_ref(), &name, &imags);
            let target_path = std::path::Path::new(&image_path);
            if target_path.exists() {
                let mut vec: Vec<u8> = Vec::new();
                let mut file = match File::open(&image_path).await {
                    Ok(file) => file,
                    Err(_err) => { panic!("{}", image_path) }
                };
                let _ = file.read_to_end(&mut vec).await.expect("read file ok");
                let content_type = match mime_guess::from_ext(&imags.format).first_raw() {
                    Some(mime) => mime,
                    None => { panic!("1234") }
                };

                (
                    axum::response::AppendHeaders([
                        (http::header::CONTENT_TYPE, content_type),
                    ]),
                    vec
                ).into_response()
            } else {
                let e = decode(imags.url);
                let y = std::str::from_utf8(&e).unwrap();
                Redirect::to(y).into_response()
            }
        } else {
            let mut buf = Vec::new();
            let filename = format!("{}.{}", name, imags.format);
            read_7z_file(imag_7z_path.as_ref(), filename.as_ref(), &mut buf);

            let content_type = match mime_guess::from_ext(&imags.format).first_raw() {
                Some(mime) => mime,
                None => { panic!("1234") }
            };

            (
                axum::response::AppendHeaders([
                    (http::header::CONTENT_TYPE, content_type),
                ]),
                buf
            ).into_response()
        }
    }
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app = Router::new()
        .route("/:short_url", get(redirect_short_url))
        .route("/css/:short_url", get(read_css_file))
        .route("/check/:short_url", get(check_image_exits))
        .route("/cshape", post(home1))
        .route("/js/:short_url", get(read_js_file));

    let addr = "0.0.0.0:5002";
    axum::Server::bind(&addr.parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
    Ok(())
}
