### Docs

- Each release also publishes `ghcr.io/dutchman-cad/dutchman:X.Y.Z-solidcraft` and
  `latest-solidcraft` for x86_64 and ARM64: the same image with SolidCraft's checker, so Docker
  users get checked histories with no build step. The default image stays small. The Compose
  example uses it for Dutchman Web.
