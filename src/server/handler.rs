use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tiny_http::{Header, Request, Response};

use super::utils::{check_assets_directory, content_type_header, determine_content_type};
use crate::template::{generate_html, generate_html_wasm_bindgen};

/// Handle an incoming HTTP request
pub fn handle_request(
    request: Request,
    js_filename: Option<&str>,
    wasm_filename: &str,
    wasm_path: &str,
    watch_mode: bool,
    clients_to_reload: &mut Vec<String>,
) {
    let url = request.url().to_string();
    let client_addr = match request.remote_addr() {
        Some(addr) => addr.to_string(),
        None => "unknown".to_string(),
    };

    println!("📝 Received request for: {}", url);

    if url == "/" {
        // Serve the main HTML page
        let html = if let Some(js_file) = js_filename {
            // This is a wasm-bindgen project
            generate_html_wasm_bindgen(js_file, wasm_filename)
        } else {
            // Regular wasm file
            generate_html(wasm_filename)
        };

        let response = Response::from_string(html).with_header(content_type_header("text/html"));
        if let Err(e) = request.respond(response) {
            eprintln!("❗ Error sending HTML response: {}", e);
        }

        if watch_mode && !clients_to_reload.contains(&client_addr) {
            clients_to_reload.push(client_addr);
        }
    } else if url == format!("/{}", wasm_filename) {
        serve_file(request, wasm_path, "application/wasm");
    } else if let Some(js_file) = js_filename {
        if url == format!("/{}", js_file) {
            let js_path = Path::new(wasm_path).parent().unwrap().join(js_file);
            serve_file(request, js_path.to_str().unwrap(), "application/javascript");
        }
    } else if url == "/reload" {
        if watch_mode {
            // TODO: check if there was an actual file change
            println!("🔄 Handling reload request in watch mode");

            let response =
                Response::from_string("no-reload").with_header(content_type_header("text/plain"));

            if let Err(e) = request.respond(response) {
                eprintln!("❗ Error sending reload response: {}", e);
            }
        } else {
            let response = Response::from_string("not-watching")
                .with_header(content_type_header("text/plain"));

            if let Err(e) = request.respond(response) {
                eprintln!("❗ Error sending reload response: {}", e);
            }
        }
    } else if url.starts_with("/assets/") {
        serve_asset(request, &url);
    } else {
        let base_dir = Path::new(wasm_path).parent().unwrap();
        let requested_file = base_dir.join(url.trim_start_matches('/'));

        if requested_file.exists() && requested_file.is_file() {
            let content_type = determine_content_type(&requested_file);
            serve_file(request, requested_file.to_str().unwrap(), content_type);
        } else {
            if url.ends_with("_bg.wasm") {
                if let Ok(entries) = fs::read_dir(base_dir) {
                    for entry in entries.flatten() {
                        let entry_path = entry.path();
                        if let Some(name) = entry_path.file_name() {
                            if name.to_string_lossy().ends_with("_bg.wasm") && entry_path.is_file()
                            {
                                serve_file(
                                    request,
                                    entry_path.to_str().unwrap(),
                                    "application/wasm",
                                );
                                return;
                            }
                        }
                    }
                }
            }

            // Check for common file patterns (js, css, etc.)
            for ext in &["js", "css", "json", "wasm"] {
                if url.ends_with(&format!(".{}", ext)) {
                    let filename = url.split('/').last().unwrap_or("");
                    if let Ok(entries) = fs::read_dir(base_dir) {
                        for entry in entries.flatten() {
                            let entry_path = entry.path();
                            if entry_path
                                .file_name()
                                .map_or(false, |name| name.to_string_lossy() == filename)
                            {
                                let content_type = match *ext {
                                    "js" => "application/javascript",
                                    "css" => "text/css",
                                    "json" => "application/json",
                                    "wasm" => "application/wasm",
                                    _ => "application/octet-stream",
                                };
                                serve_file(request, entry_path.to_str().unwrap(), content_type);
                                return;
                            }
                        }
                    }
                }
            }

            // 404 for all other requests
            let response = Response::from_string("404 Not Found")
                .with_status_code(404)
                .with_header(content_type_header("text/plain"));
            if let Err(e) = request.respond(response) {
                eprintln!("❗ Error sending 404 response: {}", e);
            }
        }
    }
}

/// Handle a request for a web application
pub fn handle_webapp_request(
    request: Request,
    html: &str,
    output_dir: &str,
    clients_to_reload: &mut Vec<String>,
    reload_flag: &Arc<AtomicBool>,
) {
    let url = request.url().to_string();

    let client_addr = match request.remote_addr() {
        Some(addr) => addr.to_string(),
        None => "unknown".to_string(),
    };

    if !url.contains("reload-check") {
        println!("📝 Request: {}", url);
    }

    if url == "/" {
        let response = Response::from_string(html).with_header(content_type_header("text/html"));
        if let Err(e) = request.respond(response) {
            eprintln!("❗ Error sending HTML response: {}", e);
        }

        if !clients_to_reload.contains(&client_addr) {
            clients_to_reload.push(client_addr);
        }
    } else if url == "/reload-check" {
        let mut response = Response::from_string("");

        response = response.with_header(
            Header::from_bytes(
                &b"Cache-Control"[..],
                &b"no-cache, no-store, must-revalidate"[..],
            )
            .unwrap(),
        );

        if reload_flag.load(Ordering::SeqCst) {
            response = response
                .with_header(Header::from_bytes(&b"X-Reload-Needed"[..], &b"true"[..]).unwrap());

            // Reset reload flag
            reload_flag.store(false, Ordering::SeqCst);
            println!("🔄 Sent reload signal to browser");
        }

        if let Err(e) = request.respond(response) {
            if !url.contains("reload-check") {
                // Don't log polling errors
                eprintln!("❗ Error sending reload-check response: {}", e);
            }
        }
    } else if url.starts_with("/assets/") {
        serve_asset(request, &url);
    } else {
        // serve from the output directory
        let file_path = Path::new(output_dir).join(url.trim_start_matches('/'));

        if file_path.exists() && file_path.is_file() {
            // Determine content type based on extension
            let content_type = determine_content_type(&file_path);
            serve_file(request, file_path.to_str().unwrap(), content_type);
        } else {
            // If the file doesn't exist, serve the main HTML page
            let response =
                Response::from_string(html).with_header(content_type_header("text/html"));
            if let Err(e) = request.respond(response) {
                eprintln!("❗ Error sending HTML response for SPA routing: {}", e);
            }
        }
    }
}

/// Serve a file
pub fn serve_file(request: Request, file_path: &str, content_type: &str) {
    match fs::read(file_path) {
        Ok(file_bytes) => {
            println!(
                "🔄 Serving file: {} ({} bytes, content-type: {})",
                file_path,
                file_bytes.len(),
                content_type
            );
            let response =
                Response::from_data(file_bytes).with_header(content_type_header(content_type));
            if let Err(e) = request.respond(response) {
                eprintln!("❗ Error sending file response: {}", e);
            }
        }
        Err(e) => {
            eprintln!("❗ Error reading file {}: {}", file_path, e);
            let response = Response::from_string(format!("Error: {}", e))
                .with_status_code(500)
                .with_header(content_type_header("text/plain"));
            if let Err(e) = request.respond(response) {
                eprintln!("❗ Error sending error response: {}", e);
            }
        }
    }
}

/// Serve a static asset file
pub fn serve_asset(request: Request, url: &str) {
    let asset_filename = url.strip_prefix("/assets/").unwrap_or("");
    let asset_path = format!("./assets/{}", asset_filename);

    let content_type = if url.ends_with(".png") {
        "image/png"
    } else if url.ends_with(".jpg") || url.ends_with(".jpeg") {
        "image/jpeg"
    } else if url.ends_with(".svg") {
        "image/svg+xml"
    } else if url.ends_with(".gif") {
        "image/gif"
    } else if url.ends_with(".css") {
        "text/css"
    } else if url.ends_with(".js") {
        "application/javascript"
    } else {
        "application/octet-stream"
    };

    match fs::read(&asset_path) {
        Ok(asset_bytes) => {
            println!(
                "🖼️ Successfully serving asset: {} ({} bytes)",
                asset_path,
                asset_bytes.len()
            );
            let response =
                Response::from_data(asset_bytes).with_header(content_type_header(content_type));
            if let Err(e) = request.respond(response) {
                eprintln!("‼️ Error sending asset response: {}", e);
            }
        }
        Err(e) => {
            eprintln!(
                "‼️ Error reading asset file {}: {} (does the file exist?)",
                asset_path, e
            );

            check_assets_directory();

            let response = Response::from_string(format!("Asset not found: {}", e))
                .with_status_code(404)
                .with_header(content_type_header("text/plain"));
            if let Err(e) = request.respond(response) {
                eprintln!("‼️ Error sending asset error response: {}", e);
            }
        }
    }
}
