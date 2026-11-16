### Docs

- The Compose example for SolidCraft and Dutchman on one address uses the published SolidCraft
  web image, so it starts in under a minute with SolidCraft 0.42 instead of compiling an old
  SolidCraft for a quarter of an hour.
- The Docker and web pages start Dutchman Web from the pulled image without rebuilding it.
- Docker docs show how to add SolidCraft's checker to the published image, without building
  Dutchman, so histories are checked; building it yourself with SolidCraft works on ARM64 too.
