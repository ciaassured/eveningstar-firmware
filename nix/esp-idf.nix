{
  perSystem = { inputs', ... }: {
    # ESP-IDF with the RISC-V toolchain (ESP32-C6). Uses nixpkgs-esp-dev's own
    # pinned nixpkgs, which permits the insecure python ecdsa package esptool needs.
    packages.esp-idf = inputs'.nixpkgs-esp-dev.packages.esp-idf-riscv;
  };
}
