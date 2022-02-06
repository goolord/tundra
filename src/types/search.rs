pub enum SearchDSL {
    Literal(String),
    Metadata(String, String),
    Negate(SearchDSL),
}
