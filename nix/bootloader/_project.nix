# Minimal ESP-IDF project. The bootloader is always built as a sub-build of an
# app project; the app itself is never compiled.
{ runCommand, partitionsCsv }:

runCommand "eveningstar-idf-project" { } ''
  mkdir -p $out/main
  cp ${partitionsCsv} $out/partitions.csv

  cat > $out/CMakeLists.txt <<'CMAKE'
  cmake_minimum_required(VERSION 3.16)
  include($ENV{IDF_PATH}/tools/cmake/project.cmake)
  idf_build_set_property(MINIMAL_BUILD ON)
  project(eveningstar)
  CMAKE

  cat > $out/main/CMakeLists.txt <<'CMAKE'
  idf_component_register(SRCS "main.c")
  CMAKE

  echo 'void app_main(void) {}' > $out/main/main.c
''
