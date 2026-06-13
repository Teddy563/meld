"""Project-wide constants. One source of truth for the block/region geometry."""

REGION_BLOCKS = 512        # blocks per Minecraft region side (32 chunks * 16)
CHUNK_BLOCKS = 16          # blocks per chunk side
REGION_CHUNKS = 32         # chunks per region side

METERS_PER_DEG_LAT = 111_320.0   # constant; latitude has no cos factor
