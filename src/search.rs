use sqlx::{Postgres, QueryBuilder};

#[derive(Debug, Default)]
pub struct Search {
    pub terms: Vec<Term>,
    pub ascending: bool,
    pub favorite_order: bool,
    pub unsupported: bool,
}
#[derive(Debug)]
pub enum Term {
    Tag(String, bool),
    Id(Vec<i64>),
    Rating(String),
    Md5(String),
    Source(String),
    Parent(Option<i64>),
    Favorite(String),
}
impl Search {
    pub fn parse(input: &str) -> Self {
        let mut search = Self::default();
        for word in input.split_whitespace() {
            if let Some((key, value)) = word.split_once(':') {
                match key {
                    "id" => match value
                        .split(',')
                        .map(str::parse::<i64>)
                        .collect::<Result<Vec<_>, _>>()
                    {
                        Ok(ids) if !ids.is_empty() => search.terms.push(Term::Id(ids)),
                        _ => search.unsupported = true,
                    },
                    "rating" => {
                        if ["g", "s", "q", "e"].contains(&value) {
                            search.terms.push(Term::Rating(value.into()));
                        } else {
                            search.unsupported = true;
                        }
                    }
                    "md5" => search.terms.push(Term::Md5(value.into())),
                    "source" => search.terms.push(Term::Source(value.into())),
                    "parent" => {
                        if value == "none" {
                            search.terms.push(Term::Parent(None));
                        } else if let Ok(id) = value.parse() {
                            search.terms.push(Term::Parent(Some(id)));
                        } else {
                            search.unsupported = true;
                        }
                    }
                    "fav" | "ordfav" => {
                        search.terms.push(Term::Favorite(value.into()));
                        search.favorite_order |= key == "ordfav";
                    }
                    "order" => match value {
                        "id" | "id_desc" => search.ascending = false,
                        "id_asc" => search.ascending = true,
                        _ => search.unsupported = true,
                    },
                    _ => search.unsupported = true,
                }
            } else {
                let (value, negative) = word
                    .strip_prefix('-')
                    .map(|w| (w, true))
                    .unwrap_or((word, false));
                if !value.is_empty() {
                    search.terms.push(Term::Tag(value.to_lowercase(), negative));
                }
            }
        }
        search
    }
    pub fn predicates(&self, sql: &mut QueryBuilder<Postgres>, owner: &str) {
        sql.push(" WHERE true");
        if self.unsupported {
            sql.push(" AND false");
        }
        for term in &self.terms {
            match term {
                Term::Tag(name, negative) => {
                    sql.push(if *negative {
                        " AND NOT EXISTS ("
                    } else {
                        " AND EXISTS ("
                    });
                    sql.push("SELECT 1 FROM post_tags pt JOIN tags t ON t.id=pt.tag_id WHERE pt.post_id=p.id AND t.name=").push_bind(name.clone()).push(")");
                }
                Term::Id(ids) => {
                    sql.push(" AND p.id=ANY(").push_bind(ids.clone()).push(")");
                }
                Term::Rating(rating) => {
                    sql.push(" AND p.rating=").push_bind(rating.clone());
                }
                Term::Md5(md5) => {
                    sql.push(" AND a.md5=").push_bind(md5.clone());
                }
                Term::Source(source) => {
                    sql.push(" AND p.source ILIKE ")
                        .push_bind(format!("%{}%", escape_like(source)));
                }
                Term::Parent(Some(id)) => {
                    sql.push(" AND p.parent_id=").push_bind(*id);
                }
                Term::Parent(None) => {
                    sql.push(" AND p.parent_id IS NULL");
                }
                Term::Favorite(user) => {
                    if !user.eq_ignore_ascii_case(owner) && user != "1" {
                        sql.push(" AND false");
                    } else {
                        sql.push(" AND EXISTS (SELECT 1 FROM favorites f WHERE f.post_id=p.id)");
                    }
                }
            }
        }
    }
}
pub fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_does_not_silently_broaden_unsupported_queries() {
        assert!(Search::parse("cat order:random").unsupported);
        assert!(Search::parse("rating:invalid").unsupported);
        assert!(Search::parse("id:1,nope").unsupported);
        assert!(!Search::parse("cat -dog rating:q id:1,2 parent:none ordfav:owner").unsupported);
    }
    #[test]
    fn literal_like_values_are_escaped() {
        assert_eq!(escape_like("a_b%\\"), "a\\_b\\%\\\\");
    }
}
