//! Precomputed search text keeps metadata processing out of the rendering loop.
use crate::fab::Item;

pub struct SearchDocument {
    title: String,
    text: String,
}
impl SearchDocument {
    pub fn new(item: &Item) -> Self {
        let title = item.title.to_lowercase();
        let mut fields = vec![title.clone()];
        if let Some(description) = &item.description {
            fields.push(visible_text(description));
        }
        if let Some(seller) = &item.seller {
            fields.push(seller.to_lowercase());
        }
        if let Some(metadata) = &item.listing_search {
            fields.push(visible_text(&metadata.description));
            fields.push(metadata.publisher.to_lowercase());
            fields.extend(metadata.tags.iter().map(|tag| tag.to_lowercase()));
        }
        Self {
            title,
            text: fields.join("\n"),
        }
    }
}

pub struct SearchQuery {
    terms: Vec<String>,
}
impl SearchQuery {
    pub fn new(query: &str) -> Self {
        Self {
            terms: query.split_whitespace().map(str::to_lowercase).collect(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
    /// All terms must match. Title matches precede broader metadata matches.
    pub fn rank(&self, document: &SearchDocument) -> Option<u8> {
        if self.terms.iter().all(|term| document.title.contains(term)) {
            Some(0)
        } else if self.terms.iter().all(|term| document.text.contains(term)) {
            Some(1)
        } else {
            None
        }
    }
}

// Index visible description text, not HTML attribute names or link destinations.
fn visible_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut inside_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                inside_tag = true;
                text.push(' ');
            }
            '>' if inside_tag => {
                inside_tag = false;
                text.push(' ');
            }
            _ if !inside_tag => text.push(c),
            _ => {}
        }
    }
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        rest = &rest[start..];
        let entity = rest
            .find(';')
            .filter(|end| *end <= 12)
            .map(|end| (&rest[1..end], end));
        let replacement = entity.and_then(|(name, end)| {
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|s| u32::from_str_radix(s, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|s| s.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, end))
        });
        if let Some((c, end)) = replacement {
            decoded.push(c);
            rest = &rest[end + 1..];
        } else {
            decoded.push('&');
            rest = &rest[1..];
        }
    }
    decoded.push_str(rest);
    decoded.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn document(title: &str, metadata: serde_json::Value) -> SearchDocument {
        let item = serde_json::from_value(json!({"assetId":"test", "assetNamespace":"test",
            "title":title, "listingSearch":metadata}))
        .unwrap();
        SearchDocument::new(&item)
    }
    #[test]
    fn title_tag_description_and_publisher_matches_are_ranked() {
        let query = SearchQuery::new("  MONSTER  ");
        assert_eq!(query.rank(&document("Wood Monster", json!({}))), Some(0));
        assert_eq!(
            query.rank(&document(
                "Paragon: Grux",
                json!({"tags":["Creature","Monster"]})
            )),
            Some(1)
        );
        assert_eq!(
            query.rank(&document(
                "Werewolf",
                json!({"description":"<p>A terrifying monster.</p>"})
            )),
            Some(1)
        );
        assert_eq!(
            query.rank(&document(
                "Survivors Roguelike",
                json!({"publisher":"Monster Tooth Studios"})
            )),
            Some(1)
        );
        assert_eq!(
            query.rank(&document("Forest", json!({"tags":["Trees"]}))),
            None
        );
    }
    #[test]
    fn all_terms_required_across_fields_and_empty_query_matches_everything() {
        let doc = document(
            "Paragon: Grux",
            json!({"tags":["Monster"],"publisher":"Epic Games"}),
        );
        assert_eq!(SearchQuery::new("grux monster epic").rank(&doc), Some(1));
        assert_eq!(SearchQuery::new("monster castle").rank(&doc), None);
        assert_eq!(SearchQuery::new("paragon grux").rank(&doc), Some(0));
        assert_eq!(SearchQuery::new(" \t ").rank(&doc), Some(0));
    }
    #[test]
    fn html_attributes_do_not_create_false_matches_and_entities_are_decoded() {
        let doc = document(
            "Forest",
            json!({"description":"<a href='https://monster.example'>Trees &amp; flowers</a><p>Stone&#32;golem</p>"}),
        );
        assert_eq!(SearchQuery::new("monster").rank(&doc), None);
        assert_eq!(SearchQuery::new("trees & flowers").rank(&doc), Some(1));
        assert_eq!(SearchQuery::new("stone golem").rank(&doc), Some(1));
    }
    #[test]
    fn library_metadata_is_searchable_before_details_arrive() {
        let item = serde_json::from_value(json!({"assetId":"test","assetNamespace":"test",
            "title":"Survivors", "seller":"Monster Tooth Studios", "description":"A roguelike template"})).unwrap();
        let doc = SearchDocument::new(&item);
        assert_eq!(SearchQuery::new("monster").rank(&doc), Some(1));
        assert_eq!(SearchQuery::new("roguelike").rank(&doc), Some(1));
    }
}
