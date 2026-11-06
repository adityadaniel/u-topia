# Screen 07 — graph view (⌘⇧G)

The source changes are covered by:

- `node_size_spans_six_to_eighteen_px` and `graph_sizes_nodes_by_links` (GR-1)
- `hover_lights_the_page_and_its_neighbours`, `graph_hover_shows_a_floating_card` and
  `graph_cmd_click_opens_a_split` (GR-2)
- `graph_legend_and_zoom_controls_sit_at_the_edges` (GR-3)
- `page_graph_global_and_local`, `edge_width_grows_with_stored_references` and
  `arrowhead_points_at_the_target` (GR-4)
- `graph_panel_sits_beside_the_page` (GR-5)

## Captures

`before.png` is the pre-change graph capture from `docs/images/graph.png`,
downscaled for this folder. The after captures were taken with `UTOPIA_SHOT_SCRIPT`
against a freshly seeded `target/redesign-07/Demo.utopia` (Roam import plus two
daily captures), with `UTOPIA_CONFIG_DIR=target/redesign-07/config`.

| File | What it proves |
|---|---|
| `before.png` | The old graph: equal 10 px dots, 1 px undirected edges, no legend or zoom controls, and a count in the top bar. |
| `graph-full.png` | Nodes sized 6–18 px by link count, the origin day ringed in accent, weighted edges with arrowheads, the colour legend and page/link count at the bottom left, and −/+/Fit at the bottom right. The top bar now holds only the filters and Close. |
| `graph-hover.png` | Hovering *Atlas Launch* lights only it and its neighbours; *Hiring* and *Weekly Review* fade to 25 %. The floating card shows the title, `6 links`, and the first block (`status:: in progress`) without moving the canvas. |
| `graph-panel.png` | "Around this page" as a right-sidebar panel beside the still-editable Daily Notes feed, with the legend/controls in a footer and *All pages* returning to full screen. |
