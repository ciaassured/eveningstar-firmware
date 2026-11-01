{
  eveningstar.board = {
    chip = "esp32c6";

    flash = {
      size = "8MB";
      mode = "dio";
      freq = "80m";
    };

    # Two OTA slots with a configuration partition each, no factory app. The
    # layout is a contract that can't change over OTA, so every offset and size
    # is written out in full: nothing is left to gen_esp32part. It is generated
    # into crates/eveningstar-storage/src/layout.rs, and generating it fails for
    # a layout whose data partitions aren't 4 KiB aligned, whose app partitions
    # aren't 64 KiB aligned, that overlaps, or that doesn't fit the flash.
    # `factory` and `reserved` hold space that is claimed later, and the
    # 0xC000-0x10000 gap keeps the first app slot 64 KiB aligned.
    partitions = [
      { name = "factory"; type = "data"; subtype = "undefined"; offset = "0x9000"; size = "0x1000"; flags = [ "readonly" ]; }
      { name = "otadata"; type = "data"; subtype = "ota"; offset = "0xA000"; size = "0x2000"; }
      { name = "ota_0"; type = "app"; subtype = "ota_0"; offset = "0x10000"; size = "0x300000"; }
      { name = "ota_1"; type = "app"; subtype = "ota_1"; offset = "0x310000"; size = "0x300000"; }
      { name = "config_0"; type = "data"; subtype = "undefined"; offset = "0x610000"; size = "0x20000"; }
      { name = "config_1"; type = "data"; subtype = "undefined"; offset = "0x630000"; size = "0x20000"; }
      { name = "diag"; type = "data"; subtype = "undefined"; offset = "0x650000"; size = "0x40000"; }
      { name = "reserved"; type = "data"; subtype = "undefined"; offset = "0x690000"; size = "0x170000"; }
    ];
  };
}
