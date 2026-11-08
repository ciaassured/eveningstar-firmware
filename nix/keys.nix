{
  # The firmware signing keys, the one place that says which key is which.
  # The trust sets in crates/eveningstar-signify/src/trust.rs and the README's
  # key table are generated from it by the `trust-sets` command.
  #
  # Releases trust the `signing` generation and the next two that aren't lost
  # or compromised; development builds trust those and the development key.
  # Until the first key ceremony there are no generations: development builds
  # trust the development key alone, and there is no release trust set.
  eveningstar.keys = {
    dev = ../keys/eveningstar-dev-fw.pub;
    generations = { };
    signing = null;
  };
}
