# Resolve the requested settings into a full sdkconfig and check every value took.
{ lib, writeText, callPackage, esp-idf, project, target, settings }:

let
  render = key: value:
    if value == true then "CONFIG_${key}=y"
    else if value == false then "# CONFIG_${key} is not set"
    else if builtins.isInt value then "CONFIG_${key}=${toString value}"
    else if builtins.match "0x[0-9a-fA-F]+" value != null then "CONFIG_${key}=${value}"
    else "CONFIG_${key}=${builtins.toJSON value}";

  defaults = writeText "sdkconfig.defaults"
    (lib.concatStringsSep "\n" (lib.mapAttrsToList render settings) + "\n");

  requested = writeText "sdkconfig-requested.json" (builtins.toJSON settings);

  idfBuild = callPackage ./_idf-build.nix { inherit esp-idf; };
in
idfBuild {
  name = "eveningstar-sdkconfig";
  inherit project target;
  sdkconfigDefaults = defaults;
  idfArgs = "reconfigure";

  installPhase = ''
    python3 ${./_check-sdkconfig.py} ${requested} build/config/sdkconfig.json

    mkdir -p $out
    cp sdkconfig $out/sdkconfig
    cp build/config/sdkconfig.json $out/sdkconfig.json
    cp sdkconfig.defaults $out/sdkconfig.defaults
  '';

  passthru = { inherit esp-idf project target settings; };
}
