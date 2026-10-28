{ lib }:
rec {
  # The files cargo reads before it looks at any crate.
  cargoConfig = lib.fileset.unions [
    ../Cargo.toml
    ../Cargo.lock
    ../.cargo
  ];

  members = lib.attrNames (lib.filterAttrs (_: type: type == "directory") (builtins.readDir ../crates));

  # Every workspace crate's manifest, which is all a dependency build needs.
  manifests = lib.fileset.fileFilter (file: file.name == "Cargo.toml") ../crates;

  # Empty stand-ins for the given workspace crates' targets, matching the ones
  # the real crate has so cargo resolves the same targets and features.
  stubsFor = stubbed: lib.concatMapStrings
    (member:
      let
        dir = ../crates + "/${member}";
        has = path: builtins.pathExists (dir + "/${path}");
      in
      ''
        mkdir -p crates/${member}/src
      ''
      + lib.optionalString (has "src/lib.rs") ''
        echo '#![no_std]' > crates/${member}/src/lib.rs
      ''
      + lib.optionalString (has "src/main.rs") ''
        cat > crates/${member}/src/main.rs <<'EOF'
        #![no_std]
        #![no_main]
        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo) -> ! {
            loop {}
        }
        EOF
      ''
      + lib.optionalString (has "build.rs") ''
        echo 'fn main() {}' > crates/${member}/build.rs
      ''
      + lib.optionalString (has "examples") (lib.concatMapStrings
        (example: ''
          mkdir -p crates/${member}/examples
          echo 'fn main() {}' > crates/${member}/examples/${example}
        '')
        (lib.attrNames (builtins.readDir (dir + "/examples")))))
    stubbed;

  stubs = stubsFor members;
}
