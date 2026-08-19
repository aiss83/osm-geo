//! Сборка полигонов границы страны из набора way-полилиний.

use crate::model::CountryBoundary;

/// Сшить открытые полилинии в замкнутые кольца по совпадающим концам.
///
/// Каждая way — упорядоченный список точек. Кольца собираются последовательным
/// соединением по совпадающим конечным точкам.
pub fn stitch_rings(mut ways: Vec<Vec<(f32, f32)>>) -> Vec<Vec<(f32, f32)>> {
    let mut rings = Vec::new();

    while let Some(first) = ways.pop() {
        if first.len() < 2 {
            continue;
        }
        let mut ring = first;
        let mut start = ring[0];
        let mut end = ring[ring.len() - 1];

        loop {
            if start == end && ring.len() > 3 {
                break;
            }

            let mut matched = false;
            for i in 0..ways.len() {
                let w = &ways[i];
                let wh = w[0];
                let wt = w[w.len() - 1];

                if wh == end {
                    ring.extend_from_slice(&w[1..]);
                    end = wt;
                    ways.swap_remove(i);
                    matched = true;
                    break;
                } else if wt == end {
                    let mut rev = w.clone();
                    rev.reverse();
                    ring.extend_from_slice(&rev[1..]);
                    end = wh;
                    ways.swap_remove(i);
                    matched = true;
                    break;
                } else if wt == start {
                    ring = w.iter().chain(ring.iter().skip(1)).cloned().collect();
                    start = wh;
                    ways.swap_remove(i);
                    matched = true;
                    break;
                } else if wh == start {
                    let rev: Vec<_> = w.iter().rev().cloned().collect();
                    ring = rev.iter().chain(ring.iter().skip(1)).cloned().collect();
                    start = wt;
                    ways.swap_remove(i);
                    matched = true;
                    break;
                }
            }

            if !matched {
                break;
            }
        }

        rings.push(ring);
    }

    rings
}

/// Ray casting: находится ли точка внутри кольца (even-odd).
pub fn point_in_ring(point: (f32, f32), ring: &[(f32, f32)]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let (x, y) = point;
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % n];
        if (y1 > y) != (y2 > y) {
            let xinters = (x2 - x1) * (y - y1) / (y2 - y1) + x1;
            if x < xinters {
                inside = !inside;
            }
        }
    }
    inside
}

/// Распределить внутренние кольца по внешним, сформировав полигоны.
pub fn build_polygons(
    outer: Vec<Vec<(f32, f32)>>,
    inner: Vec<Vec<(f32, f32)>>,
) -> Vec<Vec<Vec<(f32, f32)>>> {
    let mut polygons: Vec<Vec<Vec<(f32, f32)>>> =
        outer.into_iter().map(|ring| vec![ring]).collect();

    for hole in inner {
        let seed = hole[0];
        let mut placed = false;
        for poly in &mut polygons {
            if point_in_ring(seed, &poly[0]) {
                poly.push(hole.clone());
                placed = true;
                break;
            }
        }
        if !placed {
            // Дефенсивно: оставляем как отдельный полигон.
            polygons.push(vec![hole]);
        }
    }

    polygons
}

/// Построить `CountryBoundary` из способов внешних и внутренних колец.
pub fn build_boundary(
    outer_ways: Vec<Vec<(f32, f32)>>,
    inner_ways: Vec<Vec<(f32, f32)>>,
) -> Option<CountryBoundary> {
    let outer = stitch_rings(outer_ways);
    if outer.is_empty() {
        return None;
    }
    let inner = stitch_rings(inner_ways);
    let polygons = build_polygons(outer, inner);

    boundary_from_polygons(polygons)
}

/// Собрать `CountryBoundary` из готовых полигонов.
fn boundary_from_polygons(polygons: Vec<Vec<Vec<(f32, f32)>>>) -> Option<CountryBoundary> {
    if polygons.is_empty() {
        return None;
    }
    let mut min_lat = f32::MAX;
    let mut min_lon = f32::MAX;
    let mut max_lat = f32::MIN;
    let mut max_lon = f32::MIN;
    for poly in &polygons {
        for ring in poly {
            for &(lat, lon) in ring {
                min_lat = min_lat.min(lat);
                min_lon = min_lon.min(lon);
                max_lat = max_lat.max(lat);
                max_lon = max_lon.max(lon);
            }
        }
    }

    Some(CountryBoundary {
        min_lat,
        min_lon,
        max_lat,
        max_lon,
        polygons,
    })
}

/// Распарсить GeoJSON `FeatureCollection` (вывод `gol query -f geojson`)
/// и собрать `CountryBoundary`.
///
/// Поддерживаются geometry-типы `Polygon` и `MultiPolygon`.
pub fn parse_geojson_boundary(json: &str) -> Option<CountryBoundary> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let features = value.get("features")?.as_array()?;
    let geometry = features.first()?.get("geometry")?;
    let gtype = geometry.get("type")?.as_str()?;
    let coords = geometry.get("coordinates")?;

    let mut polygons: Vec<Vec<Vec<(f32, f32)>>> = Vec::new();

    match gtype {
        "Polygon" => {
            // coordinates = [ring]
            let rings = coords.as_array()?;
            let mut poly = Vec::with_capacity(rings.len());
            for ring in rings {
                poly.push(parse_ring(ring)?);
            }
            polygons.push(poly);
        }
        "MultiPolygon" => {
            // coordinates = [polygon] = [ [ring] ]
            for polygon in coords.as_array()? {
                let mut poly = Vec::new();
                for ring in polygon.as_array()? {
                    poly.push(parse_ring(ring)?);
                }
                polygons.push(poly);
            }
        }
        _ => return None,
    }

    boundary_from_polygons(polygons)
}

fn parse_ring(ring: &serde_json::Value) -> Option<Vec<(f32, f32)>> {
    let points = ring.as_array()?;
    let mut out = Vec::with_capacity(points.len());
    for point in points {
        let pair = point.as_array()?;
        if pair.len() < 2 {
            continue;
        }
        // GeoJSON: [lon, lat]
        let lon = pair[0].as_f64()? as f32;
        let lat = pair[1].as_f64()? as f32;
        out.push((lat, lon));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stitch_square() {
        // Квадрат из двух сегментов.
        let ways = vec![
            vec![(0.0, 0.0), (1.0, 0.0)],
            vec![(1.0, 0.0), (1.0, 1.0)],
            vec![(1.0, 1.0), (0.0, 1.0)],
            vec![(0.0, 1.0), (0.0, 0.0)],
        ];
        let rings = stitch_rings(ways);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].len(), 5); // замкнуто
        assert_eq!(rings[0].first(), rings[0].last());
    }

    #[test]
    fn test_point_in_ring() {
        let ring = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)];
        assert!(point_in_ring((1.0, 1.0), &ring));
        assert!(!point_in_ring((3.0, 3.0), &ring));
    }
}
