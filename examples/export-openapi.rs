fn main() {
    let document = agent::app::openapi_document();
    println!("{}", serde_json::to_string_pretty(&document).expect("OpenAPI document serializes"));
}
