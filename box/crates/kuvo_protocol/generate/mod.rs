use std::collections::HashSet;
use std::fmt::Write;

use crate::Answers;
use crate::shape::{Compound, Literal, Primitive, Shape};

mod names;
use names::{Names, pascal, snake};

pub fn routes(answers: Answers) -> Result<String, String> {
    let mut generator = Generator::default();
    let refusal = generator.intern(&answers.error, "Refusal");
    let mut modules = String::new();
    let mut table = String::new();
    let mut module_names = HashSet::new();

    for route in answers.routes {
        let Some((method, path)) = route.route.split_once(' ') else {
            return Err(format!("route {:?} has no method and path", route.route));
        };
        let module = snake(&route.route);
        // Keep the public module path predictable and report a collision with context.
        if !module_names.insert(module.clone()) {
            return Err(format!(
                "route {:?} produces duplicate Rust module {module}",
                route.route
            ));
        }
        writeln!(modules, "pub mod {module} {{").unwrap();
        writeln!(
            table,
            "crate::Route {{ method: {method:?}, path: {path:?}, feature: {:?}, answers: &[",
            route.feature
        )
        .unwrap();
        for (status, shape) in route.answers {
            let id = generator.intern(&shape, &format!("{}Status{status}", pascal(&route.route)));
            let ty = &generator.shapes[id].ty;
            writeln!(modules, "pub type Status{status} = {ty};").unwrap();
            writeln!(table, "({status}, &SHAPE_{id}),").unwrap();
        }
        match &route.body {
            Some(shape) => {
                let id = generator.intern(shape, &format!("{}Request", pascal(&route.route)));
                writeln!(modules, "pub type Request = {};", generator.shapes[id].ty).unwrap();
                writeln!(table, "], body: Some(&SHAPE_{id}) }},").unwrap();
            }
            None => table.push_str("], body: None },\n"),
        }
        modules.push_str("}\n");
    }

    let mut output = String::from("// Generated from the TypeScript route export.\n");
    writeln!(
        output,
        "#[allow(clippy::large_enum_variant, clippy::type_complexity)] pub mod types {{ {} }}",
        generator.types
    )
    .unwrap();
    output.push_str(&generator.checks);
    let refusal_type = &generator.shapes[refusal].ty;
    writeln!(
        output,
        "pub type Refusal = {refusal_type};
        pub const REFUSAL_SHAPE: &crate::shape::Shape = &SHAPE_{refusal};
        pub const PASSED_ON: &[&str] = &{:?};
        pub const ROUTES: &[crate::Route] = &[{table}];",
        answers.passed_on
    )
    .unwrap();
    output.push_str(&modules);
    Ok(output)
}

struct Entry {
    shape: Shape,
    ty: String,
}

#[derive(Default)]
struct Generator {
    shapes: Vec<Entry>,
    names: Names,
    types: String,
    checks: String,
}

impl Generator {
    fn intern(&mut self, shape: &Shape, hint: &str) -> usize {
        if let Some(id) = self.shapes.iter().position(|entry| entry.shape == *shape) {
            return id;
        }
        let (ty, check) = self.generate(shape, hint);
        let id = self.shapes.len();
        self.shapes.push(Entry {
            shape: shape.clone(),
            ty,
        });
        writeln!(
            self.checks,
            "static SHAPE_{id}: crate::shape::Shape = {check};"
        )
        .unwrap();
        id
    }

    fn generate(&mut self, shape: &Shape, hint: &str) -> (String, String) {
        let (ty, check) = match shape {
            Shape::Primitive(primitive) => {
                let ty = match primitive {
                    Primitive::String => "String",
                    Primitive::Number => "serde_json::Number",
                    Primitive::Bool => "bool",
                    Primitive::Null => "crate::json::Null",
                    Primitive::Any => "serde_json::Value",
                    Primitive::Bytes => "Vec<u8>",
                };
                return (
                    ty.into(),
                    format!(
                        "crate::shape::Shape::Primitive(crate::shape::Primitive::{primitive:?})"
                    ),
                );
            }
            Shape::Compound(Compound::Literal(literal)) => {
                let (ty, value) = match literal {
                    Literal::String(value) => (
                        self.words(hint, &[value]),
                        format!("String(std::borrow::Cow::Borrowed({value:?}))"),
                    ),
                    Literal::Number(value) => (
                        format!("crate::json::NumberLiteral<{}>", value.to_bits()),
                        format!("Number({value:?})"),
                    ),
                    Literal::Bool(value) => (
                        format!("crate::json::BoolLiteral<{value}>"),
                        format!("Bool({value})"),
                    ),
                    Literal::Null => ("crate::json::Null".into(), "Null".into()),
                };
                (ty, format!("Literal(crate::shape::Literal::{value})"))
            }
            Shape::Compound(Compound::List(child) | Compound::Map(child)) => {
                let id = self.intern(child.shape(), &format!("{hint}Item"));
                let child_type = &self.shapes[id].ty;
                match shape {
                    Shape::Compound(Compound::List(_)) => (
                        format!("Vec<{child_type}>"),
                        format!("List(crate::shape::Child::Static(&SHAPE_{id}))"),
                    ),
                    _ => (
                        format!("std::collections::BTreeMap<String, {child_type}>"),
                        format!("Map(crate::shape::Child::Static(&SHAPE_{id}))"),
                    ),
                }
            }
            Shape::Compound(Compound::Object(fields)) => {
                let name = self.names.take(hint);
                let mut names = Names::default();
                let mut checks = String::new();
                let mut members = String::new();
                for field in fields.0.iter() {
                    let wire_name = &field.name;
                    let field_hint = format!("{name}{}", pascal(wire_name));
                    let id = self.intern(field.shape.shape(), &field_hint);
                    let ty = &self.shapes[id].ty;
                    let member = names.take(&snake(wire_name));
                    writeln!(checks, "crate::shape::Field {{ name: std::borrow::Cow::Borrowed({wire_name:?}), optional: {}, shape: crate::shape::Child::Static(&SHAPE_{id}) }},", field.optional).unwrap();
                    writeln!(members, "#[serde(rename = {wire_name:?})]").unwrap();
                    if field.optional {
                        writeln!(members, r#"#[serde(default, deserialize_with = "crate::json::present", skip_serializing_if = "Option::is_none")]
                            pub {member}: Option<{ty}>,"#).unwrap();
                    } else {
                        writeln!(members, "pub {member}: {ty},").unwrap();
                    }
                }
                writeln!(
                    self.types,
                    "crate::json::wire_object!({name}, {name:?}, {{ {members} }});"
                )
                .unwrap();
                (
                    format!("crate::routes::types::{name}"),
                    format!(
                        "Object(crate::shape::Fields(std::borrow::Cow::Borrowed(&[{checks}])))"
                    ),
                )
            }
            Shape::Compound(Compound::OneOf(choices)) => {
                let mut ids = String::new();
                let mut members = String::new();
                let words: Option<Vec<&str>> = choices
                    .iter()
                    .map(|choice| match choice.shape() {
                        Shape::Compound(Compound::Literal(Literal::String(word))) => {
                            Some(word.as_ref())
                        }
                        _ => None,
                    })
                    .collect();
                let ty = if let [first, second] = choices.as_ref()
                    && let Some(non_null) = nullable(first.shape(), second.shape())
                {
                    for choice in choices.iter() {
                        let id = self.intern(choice.shape(), hint);
                        write!(ids, "crate::shape::Child::Static(&SHAPE_{id}),").unwrap();
                    }
                    let id = self.intern(non_null, hint);
                    format!("crate::json::Nullable<{}>", self.shapes[id].ty)
                } else if let Some(words) = words.filter(|words| !words.is_empty()) {
                    for choice in choices.iter() {
                        let id = self.intern(choice.shape(), hint);
                        write!(ids, "crate::shape::Child::Static(&SHAPE_{id}),").unwrap();
                    }
                    self.words(hint, &words)
                } else {
                    let name = self.names.take(hint);
                    let mut variants = Names::default();
                    for choice in choices.iter() {
                        let variant = variants.take(&variant_name(choice.shape()));
                        let id = self.intern(choice.shape(), &format!("{name}{variant}"));
                        write!(ids, "crate::shape::Child::Static(&SHAPE_{id}),").unwrap();
                        let ty = &self.shapes[id].ty;
                        writeln!(members, "{variant}({ty}),").unwrap();
                    }
                    writeln!(
                        self.types,
                        r#"
                        #[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
                        #[serde(untagged)]
                        pub enum {name} {{ {members} }}"#
                    )
                    .unwrap();
                    format!("crate::routes::types::{name}")
                };
                (ty, format!("OneOf(std::borrow::Cow::Borrowed(&[{ids}]))"))
            }
        };
        (
            ty,
            format!("crate::shape::Shape::Compound(crate::shape::Compound::{check})"),
        )
    }

    fn words(&mut self, hint: &str, words: &[&str]) -> String {
        let name = self.names.take(hint);
        let mut variants = Names::default();
        writeln!(self.types, "#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)] pub enum {name} {{").unwrap();
        let mut seen = HashSet::new();
        for word in words.iter().filter(|word| seen.insert(**word)) {
            let variant = variants.take(&pascal(word));
            writeln!(self.types, "#[serde(rename = {word:?})] {variant},").unwrap();
        }
        self.types.push_str("}\n");
        format!("crate::routes::types::{name}")
    }
}

fn nullable<'a>(first: &'a Shape, second: &'a Shape) -> Option<&'a Shape> {
    let null = Shape::Primitive(Primitive::Null);
    if *first == null {
        Some(second)
    } else if *second == null {
        Some(first)
    } else {
        None
    }
}

fn variant_name(shape: &Shape) -> String {
    match shape {
        Shape::Primitive(kind) => format!("{kind:?}"),
        Shape::Compound(Compound::Literal(Literal::String(word))) => pascal(word),
        Shape::Compound(Compound::Literal(Literal::Bool(true))) => "True".into(),
        Shape::Compound(Compound::Literal(Literal::Bool(false))) => "False".into(),
        Shape::Compound(Compound::Map(_)) => "Map".into(),
        Shape::Compound(Compound::List(_)) => "List".into(),
        Shape::Compound(Compound::Object(fields)) => {
            for key in ["kind", "type", "state", "ok", "status"] {
                if let Some(field) = fields
                    .0
                    .iter()
                    .find(|field| !field.optional && field.name == key)
                {
                    match field.shape.shape() {
                        Shape::Compound(Compound::Literal(Literal::String(word))) => {
                            return pascal(word);
                        }
                        Shape::Compound(Compound::Literal(Literal::Bool(true))) => {
                            return "Success".into();
                        }
                        Shape::Compound(Compound::Literal(Literal::Bool(false))) => {
                            return "Failure".into();
                        }
                        _ => {}
                    }
                }
            }
            "Object".into()
        }
        _ => "Alternative".into(),
    }
}
