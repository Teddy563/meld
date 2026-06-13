"""light-meld — slim OSM→Minecraft tiled-world pipeline.

See ../../light-docs/ for the full specification. The load-bearing invariant is
in `coords.py`: a longitude/latitude maps to a Minecraft block using a
metres-per-degree constant anchored at the project ORIGIN latitude — the same
rule the Arnis fork obeys after the transform_point fix
(light-docs/03-seam-merge-and-unrendered-regions.md).
"""

__version__ = "0.1.0"
