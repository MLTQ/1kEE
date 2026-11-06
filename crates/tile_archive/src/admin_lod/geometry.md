Computes nested boundary detail tiers with an iterative Douglas–Peucker hierarchy.
Errors are measured in unwrapped longitude/latitude degrees. Endpoints and three
anchors of closed rings survive every tier. Islands, holes and open fragments are
processed separately. FULL uses every source coordinate. Cameras choose the coarsest
error below three quarters of a display pixel.
