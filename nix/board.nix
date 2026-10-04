{
  eveningstar.board = {
    chip = "esp32c6";

    flash = {
      size = "8MB";
      mode = "dio";
      freq = "80m";
    };

    # Two OTA slots, no factory app. Leaves ~1.9MB free at the end of flash.
    partitions = [
      { name = "nvs"; type = "data"; subtype = "nvs"; size = "0x4000"; }
      { name = "otadata"; type = "data"; subtype = "ota"; size = "0x2000"; }
      { name = "phy_init"; type = "data"; subtype = "phy"; size = "0x1000"; }
      { name = "ota_0"; type = "app"; subtype = "ota_0"; size = "3M"; }
      { name = "ota_1"; type = "app"; subtype = "ota_1"; size = "3M"; }
    ];
  };
}
