use std::fs;
use clap::Parser;
use inquire::{Text, Editor};
use anyhow::Result;
use convert_case::{Case, Casing};
use openapiv3::{OpenAPI, ParameterSchemaOrContent, RefOr, Schema};
use openapiv3 as oa;
use serde_json::Value;
use indexmap::map::Entry;

#[derive(Parser, Debug)]
pub struct Insert {
    pub target: String,
}

fn is_primitive(schema: &oa::Schema) -> bool {
    match &schema.kind {
        oa::SchemaKind::Type(oa::Type::Object(o)) => o.properties.is_empty(),
        oa::SchemaKind::Type(oa::Type::Array(a)) => a.items.is_none(),
        _ => true,
    }
}

fn create_schema(
    components: &oa::Components,
    value: &Value,
) -> Result<(oa::Schema, Vec<(String, oa::Schema)>)> {
    let mut deps = Vec::new();
    let s = match value {
        Value::Null => oa::Schema::new_object(),
        Value::Bool(_) => oa::Schema::new_bool(),
        Value::Number(n) => {
            let s = if n.is_f64() {
                oa::Schema::new_number()
            } else {
                oa::Schema::new_integer()
            };
            s
        }
        Value::String(_) => {
            let s = Schema::new_string();
            s
        }
        Value::Array(inner) => {
            if inner.len() == 0 {
                return Ok((Schema::new_array_any(), deps));
            };
            let (inner, dep_deps) = create_schema(components, &inner[0])?;
            deps.extend(dep_deps);
            if is_primitive(&inner) {
                Schema::new_array(inner)
            } else {
                let object_name = "Inner".to_string();
                deps.push((object_name.clone(), inner));
                Schema::new_array(RefOr::schema_ref(&object_name))
            }
        }
        Value::Object(map) => {
            let mut s = Schema::new_object();
            for (key, value) in map {
                let name = key.to_case(Case::Pascal);
                let (schema, dep_deps) = create_schema(components, value)?;
                if !is_primitive(&schema) {
                    deps.push((name.clone(), schema.clone()));
                    deps.extend(dep_deps);
                }
                s.add_required(key);
                if is_primitive(&schema) {
                    s.properties_mut().insert(key, schema);
                } else {
                    s.properties_mut().insert(key, RefOr::schema_ref(&name));
                }
            }
            s
        }
    };
    Ok((s, deps))
}

fn query_param(name: impl Into<String>) -> RefOr<oa::Parameter> {
    RefOr::Item(oa::Parameter {
        kind: oa::ParameterKind::Query {
            style: oa::QueryStyle::Form,
            allow_reserved: false,
            allow_empty_value: None,
        },
        data: oa::ParameterData {
            name: name.into(),
            description: None,
            required: false,
            deprecated: None,
            format: ParameterSchemaOrContent::Schema(RefOr::Item(Schema::new_string())),
            example: None,
            examples: Default::default(),
            explode: None,
            extensions: Default::default(),
        },
    })
}

impl Insert {
    fn insert_dependent_schemas(&self, deps: Vec<(String, Schema)>, spec: &mut OpenAPI) {
        for (name, schema) in deps {
            match spec.components.schemas.entry(name.clone()) {
                Entry::Occupied(_) => {
                    eprintln!("Schema already exists: {}", name);
                }
                Entry::Vacant(e) => {
                    e.insert(schema.into());
                }
            }
        }
    }

    fn insert_url(&self, url: String, spec: &mut OpenAPI) -> Result<()> {
        let mut url = munge_url(&url, &spec);
        let mut op = oa::Operation::default();
        let method = loop {
            let method = Text::new("What http method?").with_default("get").prompt()?;
            match method.as_str() {
                "get" => break method,
                "post" => break method,
                "put" => break method,
                "delete" => break method,
                _ => {
                    println!("Invalid method");
                    continue;
                }
            };
        };
        let operation_id = Text::new("Name the operation id:").prompt()?;
        if !operation_id.is_empty() {
            op.operation_id = Some(operation_id);
        }

        let path_args: Vec<_> = url.match_indices('{').collect();
        for (idx, _) in path_args {
            let end_idx = url[idx..].find('}').unwrap() + idx;
            let arg = &url[idx..=end_idx];
            let param_name = &arg[1..arg.len() - 1];
            let param = oa::Parameter {
                kind: oa::ParameterKind::Path {
                    style: oa::PathStyle::Simple,
                },
                data: oa::ParameterData {
                    name: param_name.to_string(),
                    description: None,
                    required: true,
                    deprecated: None,
                    format: ParameterSchemaOrContent::Schema(RefOr::Item(Schema::new_string())),
                    example: None,
                    examples: Default::default(),
                    explode: None,
                    extensions: Default::default(),
                },
            };
            op.parameters.push(RefOr::Item(param));
        }

        // check url for spaces or for ? and then add query parameters.
        if url.contains(' ') {
            for split in url.split(' ').skip(1) {
                let split = split.split_once('=')
                    .map(|(k, _)| k)
                    .unwrap_or(split);
                let param = query_param(split);
                op.parameters.push(param);
            }
            url = url.split(' ').next().unwrap().to_string();
        } else if url.contains('?') {
            let query = url.split_once('?').unwrap().1;
            for split in query.split('&') {
                let split = split.split_once('=')
                    .map(|(k, _)| k)
                    .unwrap_or(split);
                let param = query_param(split);
                op.parameters.push(param);
            }
            url = url.split('?').next().unwrap().to_string();
        } else if matches!(method.as_str(), "get") {
            loop {
                let param = Text::new("Enter a query param (blank to skip):").prompt()?;
                if param.is_empty() {
                    break;
                }
                let param = query_param(param);
                op.parameters.push(param);
            }
        }

        if matches!(method.as_str(), "put" | "post") {
            let request_body = Editor::new("What is the request body?").with_file_extension(".json").with_predefined_text(r#"{"$comment": "Replace this JSON with the request body"}"#).prompt_immediate()?;
            let request_body = serde_json::from_str(&request_body)?;
            let (schema, deps) = create_schema(&spec.components, &request_body)?;
            op.add_request_body_json(Some(RefOr::Item(schema)));
            self.insert_dependent_schemas(deps, spec);
        }

        let response_body = Editor::new("What is the response body?").with_file_extension(".json").with_predefined_text(r#"{"$comment": "Replace this JSON with the response body"}"#).prompt_immediate()?;
        let response_body = serde_json::from_str(&response_body)?;
        let (schema, deps) = create_schema(&spec.components, &response_body)?;
        op.add_response_success_json(Some(RefOr::Item(schema)));
        self.insert_dependent_schemas(deps, spec);

        let path = spec.paths.paths.entry(url.clone()).or_insert_with(Default::default).as_mut().unwrap();
        match method.as_str() {
            "get" => path.get = Some(op),
            "post" => path.post = Some(op),
            "put" => path.put = Some(op),
            "delete" => path.delete = Some(op),
            _ => panic!("Invalid method"),
        }
        Ok(())
    }

    fn insert_schema(&self, name: String, spec: &mut OpenAPI) -> Result<()> {
        let body = Editor::new("What is the schema body?").with_file_extension(".json").with_predefined_text(r#"{"$comment": "Replace this JSON with the schema body."}"#).prompt_immediate()?;
        let body = serde_json::from_str(&body)?;
        let (schema, deps) = create_schema(&spec.components, &body)?;
        self.insert_dependent_schemas(deps, spec);
        spec.schemas.insert(name, schema);
        Ok(())
    }

    pub fn run(&self) -> Result<()> {
        let mut spec: OpenAPI = serde_yaml::from_reader(fs::File::open(&self.target)?)?;
        let text = Text::new("What do you want to insert? start with slash for a URL.").prompt()?;
        if text.starts_with(['/', ':']) {
            self.insert_url(text, &mut spec)?;
        } else {
            self.insert_schema(text, &mut spec)?;
        }
        serde_yaml::to_writer(fs::File::create(&self.target)?, &spec)?;
        Ok(())
    }
}

fn munge_url(url: &str, spec: &OpenAPI) -> String {
    let mut url = url.to_string();
    if url.starts_with(":") {
        // only take the part of url including the first /
        let idx = url.find('/').unwrap();
        url = url.split_at(idx).1.to_string();
    }
    for server in &spec.servers {
        // get the path part of a url that looks like  http://localhost:5000/v1/api
        // 8 is the hacky way to skip the http(s)?:// part
        let Some(idx) = server.url[8..].find('/') else {
            continue;
        };
        let path = &server.url[8 + idx..];
        if url.starts_with(path) {
            url = url.trim_start_matches(path).to_string();
            break;
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use std_ext::default;
    #[test]
    fn test_munge_url() {
        let spec = OpenAPI {
            servers: vec![oa::Server {
                url: "http://localhost:5000/v1/api".to_string(),
                ..default()
            }],
            ..default()
        };
        assert_eq!(munge_url("/v1/api/iserver/account/{}/summary", &spec), "/iserver/account/{}/summary");
        assert_eq!(munge_url(":5000/v1/api/iserver/account/{}/summary", &spec), "/iserver/account/{}/summary");
        assert_eq!(munge_url("/iserver/account/{}/summary", &spec), "/iserver/account/{}/summary");
    }
}