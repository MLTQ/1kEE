# feature_heights.rs

## Purpose
Prepares road/waterway vertex elevations while preserving valid pre-baked heights.
The sampler is called only for missing/non-finite height entries. Road thinning
uses source indices, keeping coordinates and heights aligned.

## Contracts
- Output retains the existing endpoint/stride selection and display offset.
- Missing or mismatched elevation arrays use the runtime terrain sampler.
- Empty input is empty. At least two vertices are allowed for nonempty roads.
- Unit tests verify no sampler calls for baked geometry and index alignment.
