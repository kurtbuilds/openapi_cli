use std::fs;
use clap::Parser;
use inquire::{Text, Editor};
use anyhow::Result;
use convert_case::{Case, Casing};
use openapiv3::{OpenAPI, RefOr};
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
            let s = oa::Schema::new_string();
            s
        }
        Value::Array(inner) => {
            let inner = if inner.len() == 0 {
                oa::Schema::new_any()
            } else {
                let (schema, dep_deps) = create_schema(components, &inner[0])?;
                deps.extend(dep_deps);
                schema
            };
            if is_primitive(&inner) {
                oa::Schema::new_array(inner)
            } else {
                let object_name = "Inner".to_string();
                deps.push((object_name.clone(), inner));
                oa::Schema::new_array(RefOr::schema_ref(&object_name))
            }
        }
        Value::Object(map) => {
            let mut s = oa::Schema::new_object();
            for (key, value) in map {
                let name = key.to_case(Case::Pascal);
                let (schema, dep_deps) = create_schema(components, value)?;
                deps.push((name.clone(), schema.clone()));
                deps.extend(dep_deps);
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

    fn insert_url(&self, url: String) -> Result<()> {
        let mut spec: OpenAPI = serde_yaml::from_reader(fs::File::open(&self.target)?)?;
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
        let response_body = Editor::new("What is the response body?").with_file_extension("json").prompt_immediate()?;
        let response_body = serde_json::from_str(&response_body)?;
        let (schema, deps) = create_schema(&spec.components, &response_body)?;
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
        op.add_response_success_json(Some(RefOr::Item(schema)));
        serde_yaml::to_writer(fs::File::create(&self.target)?, &spec)?;
        Ok(())
    }

    fn insert_schema(&self, _schema: &str) -> Result<()> {
        Ok(())
    }

    pub fn run(&self) -> Result<()> {
        let text = Text::new("What do you want to insert? start with slash for a URL.").prompt()?;
        if text.starts_with('/') {
            self.insert_url(text)
        } else {
            self.insert_schema(&text)
        }
    }
}