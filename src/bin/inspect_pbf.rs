//! Быстрый инспектор PBF: ищет элементы с заданными тегами.
use osmpbf::{Element, ElementReader};
use std::collections::HashMap;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("Usage: inspect_pbf <file.pbf>");
    let path = Path::new(&path);

    let reader = ElementReader::from_path(path).unwrap();
    let mut count = 0u64;

    reader.for_each(|element| {
        let tags: HashMap<String, String> = match &element {
            Element::Node(n) => n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            Element::DenseNode(n) => n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            Element::Way(w) => w.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            Element::Relation(r) => r.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        };

        let name = tags.get("name").or_else(|| tags.get("name:ru"));
        let city = tags.get("addr:city");
        let street = tags.get("addr:street");
        let hn = tags.get("addr:housenumber");

        let place_val = tags.get("place");
        let name_match = name.map_or(false, |n| n.contains("Исаков") || n.contains("Московский проспект"));
        let city_match = city.map_or(false, |c| {
            c.contains("Исаков") || c.contains("Гурьев") || c.contains("Балтийск")
            || c.contains("Славск") || c.contains("Гвардейск") || c.contains("Снти")
        });
        let place_match = place_val.map_or(false, |_| {
            name.map_or(false, |n| n.contains("Гурьев") || n.contains("Балтийск")
                || n.contains("Славск") || n.contains("Гвардейск"))
        });
        let street_match = street.map_or(false, |s| s.contains("Московский проспект") || s.contains("проспекту"));

        if name_match || city_match || street_match || place_match {
            count += 1;
            if count <= 30 {
                let elem_type = match &element {
                    Element::Node(_) | Element::DenseNode(_) => "Node",
                    Element::Way(_) => "Way",
                    Element::Relation(_) => "Relation",
                };
                println!("[{count}] {elem_type}:");
                if let Some(n) = name { println!("  name        = {n:?}"); }
                if let Some(v) = tags.get("highway") { println!("  highway     = {v:?}"); }
                if let Some(v) = tags.get("amenity") { println!("  amenity     = {v:?}"); }
                if let Some(v) = city { println!("  addr:city   = {v:?}"); }
                if let Some(v) = street { println!("  addr:street = {v:?}"); }
                if let Some(v) = hn { println!("  addr:housenumber = {v:?}"); }
                if let Some(v) = tags.get("place") { println!("  place       = {v:?}"); }
                if let Some(v) = tags.get("historic") { println!("  historic    = {v:?}"); }
                println!();
            }
        }
    }).unwrap();

    if count > 30 {
        println!("... и ещё {} элементов", count - 30);
    }
    println!("Всего найдено: {count}");
}
