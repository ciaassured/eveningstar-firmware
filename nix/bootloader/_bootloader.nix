# Build the ESP-IDF 2nd stage bootloader from a resolved sdkconfig.
{ callPackage, sdkconfig }:

let
  inherit (sdkconfig) esp-idf project target;
  idfBuild = callPackage ./_idf-build.nix { inherit esp-idf; };
in
idfBuild {
  name = "eveningstar-bootloader-${target}-idf-${esp-idf.version}";
  inherit project target;
  sdkconfig = "${sdkconfig}/sdkconfig";
  idfArgs = "bootloader";

  installPhase = ''
    # The resolved config must pass through the build unchanged.
    cmp sdkconfig ${sdkconfig}/sdkconfig

    mkdir -p $out
    cp build/bootloader/bootloader.{bin,elf,map} $out/
    cp sdkconfig $out/sdkconfig
  '';
}
