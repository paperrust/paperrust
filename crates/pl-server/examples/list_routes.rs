//! Prints every route the server knows: `id<TAB>method<TAB>path`, sorted by id.
//! Used by the coordinator to keep routes.toml complete.
fn main() {
    let mut v: Vec<_> = pl_server::all_routes()
        .into_iter()
        .map(|r| format!("{}\t{}\t{}", r.id, r.method, r.path))
        .collect();
    v.sort();
    for l in v {
        println!("{l}");
    }
}
