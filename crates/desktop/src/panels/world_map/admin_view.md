Selects an angular error from actual physical pixel scale, with a padded snapped
geographic envelope. Globe uses the closest projection scale and a cap enclosing
the viewport. A distant disk uses the stable world overview. Local inverts the
same affine ground projection used to draw the map, including oblique and rotated
views. Only the tier, envelope and shading enter the worker key.
