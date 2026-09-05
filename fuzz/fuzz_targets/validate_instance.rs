//! Arbitrary XML into the instance validator, against a real schema.
//!
//! The validator walks a content automaton driven by attacker-controlled
//! element order, so its element stack and matcher state are reachable from
//! the document rather than only from the schema.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;
use xsdkit::instance::PsviEvent;
use xsdkit::{SchemaSetBuilder, Schemas};

/// One schema, compiled once: the fuzzer is exercising the *validator*, and
/// recompiling per input would spend every cycle in the loader instead.
fn schema() -> &'static Schemas {
    static S: OnceLock<Schemas> = OnceLock::new();
    S.get_or_init(|| {
        SchemaSetBuilder::new()
            .text(
                r###"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                              xmlns:tns="urn:f" targetNamespace="urn:f"
                              elementFormDefault="qualified">
                     <xs:simpleType name="Code">
                       <xs:restriction base="xs:string">
                         <xs:pattern value="[A-Z]{2}[0-9]+"/>
                       </xs:restriction>
                     </xs:simpleType>
                     <xs:element name="head" type="xs:string" abstract="true"/>
                     <xs:element name="leaf" type="xs:int" substitutionGroup="tns:head"/>
                     <xs:element name="root">
                       <xs:complexType>
                         <xs:sequence>
                           <xs:element name="when" type="xs:dateTime"/>
                           <xs:element name="code" type="tns:Code" minOccurs="0"/>
                           <xs:element ref="tns:head" maxOccurs="unbounded"/>
                           <xs:choice minOccurs="0">
                             <xs:element name="a" type="xs:decimal"/>
                             <xs:element name="b" type="xs:boolean"/>
                           </xs:choice>
                           <xs:any namespace="##other" processContents="lax" minOccurs="0"/>
                         </xs:sequence>
                         <xs:attribute name="id" type="xs:ID" use="required"/>
                         <xs:attribute name="n" type="xs:unsignedLong"/>
                       </xs:complexType>
                     </xs:element>
                   </xs:schema>"###,
                "fuzz://schema.xsd",
            )
            .build()
            .expect("the fuzz schema must compile")
    })
}

fuzz_target!(|data: &str| {
    if data.len() > 64 * 1024 {
        return;
    }
    let s = schema();
    // Every id the PSVI hands out must resolve in the schema it came from.
    // The document chooses the path through the automaton — a `skip` wildcard,
    // an `xsi:type` override, a schema-supplied default — so the ids reaching
    // a consumer are attacker-influenced in a way the loader's are not.
    let report = s.instance_validator().validate_with(data, |e| match e {
        PsviEvent::StartElement {
            name,
            declaration,
            type_id,
            attributes,
            ..
        } => {
            let _ = s.display_name(name);
            let _ = declaration.map(|d| s[d].name);
            let _ = s[type_id].name();
            for a in attributes {
                let _ = s.display_name(a.name);
                let _ = a.declaration.map(|d| s[d].name);
                let _ = a.value.map(|v| v.to_string());
            }
        }
        PsviEvent::Text {
            value,
            type_id,
            lexical,
            ..
        } => {
            let _ = s[type_id].name();
            if let Some(v) = value {
                let _ = v.partial_cmp_value(&v);
                let _ = v.to_string();
            }
            let _ = lexical.len();
        }
        PsviEvent::EndElement {
            name, declaration, ..
        } => {
            let _ = s.display_name(name);
            let _ = declaration.map(|d| s[d].name);
        }
        // `PsviEvent` is `#[non_exhaustive]`; a new variant should not be a
        // build failure here, but it will be unfuzzed until it is added above.
        _ => {}
    });
    // Rendering every diagnostic is part of the contract: a validator that
    // cannot describe what it rejected is not usable.
    for d in report.diagnostics.iter() {
        let _ = d.to_string();
    }
});
