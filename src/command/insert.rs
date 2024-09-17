use std::fs;
use clap::Parser;
use inquire::{Text, Editor};
use anyhow::Result;
use convert_case::{Case, Casing};
use openapiv3::{OpenAPI, RefOr, Schema};
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

impl Insert {

    fn insert_dependent_schemas(&self, deps: Vec<(String, oa::Schema)>, spec: &mut OpenAPI) {
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
        let path = spec.paths.paths.entry(url).or_insert_with(Default::default).as_mut().unwrap();
        let op = loop {
            let method = Text::new("What http method?").with_default("get").prompt()?;
            match method.as_str() {
                "get" => break path.get.get_or_insert_with(Default::default),
                "post" => break path.post.get_or_insert_with(Default::default),
                "put" => break path.put.get_or_insert_with(Default::default),
                "delete" => break path.delete.get_or_insert_with(Default::default),
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

        // let parameters = Text::new("What are the parameters?").prompt()?;
        // let request_body = Text::new("What is the request body?").prompt()?;
        let response_body = Editor::new("What is the response body?").with_file_extension(".json").with_predefined_text(r#"{"$comment": "Replace this JSON with the response body"}"#).prompt_immediate()?;
        let response_body = serde_json::from_str(&response_body)?;
        let (schema, deps) = create_schema(&spec.components, &response_body)?;
        op.add_response_success_json(Some(RefOr::Item(schema)));
        self.insert_dependent_schemas(deps, spec);
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
        if text.starts_with('/') {
            self.insert_url(text, &mut spec)?;
        } else {
            self.insert_schema(text, &mut spec)?;
        }
        serde_yaml::to_writer(fs::File::create(&self.target)?, &spec)?;
        Ok(())
    }
}