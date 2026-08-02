fn main() {
    let source_directory = std::path::Path::new("src");
    let parser_path = source_directory.join("parser.c");

    let mut compiler = cc::Build::new();
    compiler.std("c11").include(source_directory);
    #[cfg(target_env = "msvc")]
    compiler.flag("-utf-8");
    compiler.file(&parser_path);
    compiler.compile("tree-sitter-papyrus");

    println!("cargo:rerun-if-changed={}", parser_path.display());
}
