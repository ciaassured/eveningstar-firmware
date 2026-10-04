# Run idf.py against the project in the sandbox.
{ stdenv, esp-idf }:

{ name, project, target, sdkconfig ? null, sdkconfigDefaults ? null, idfArgs, installPhase, passthru ? { } }:

stdenv.mkDerivation {
  inherit name installPhase passthru;

  buildInputs = [ esp-idf ];

  dontUnpack = true;
  dontConfigure = true;
  dontFixup = true;

  buildPhase = ''
    runHook preBuild

    cp -r ${project} project
    chmod -R u+w project
    cd project

    ${if sdkconfig != null then "cp ${sdkconfig} sdkconfig && chmod u+w sdkconfig" else ""}
    ${if sdkconfigDefaults != null then "cp ${sdkconfigDefaults} sdkconfig.defaults" else ""}

    # The build system wants a writable home, and the component manager wants the network.
    export HOME=$(mktemp -d)
    export IDF_COMPONENT_MANAGER=0
    export IDF_TARGET=${target}

    idf.py ${idfArgs}

    runHook postBuild
  '';
}
