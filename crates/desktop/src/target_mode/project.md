# Target point coordinate transforms

Transforms a bounded batch of at most 32,768 source XY coordinates through the
configured `gdaltransform`, preserving float64 input and parsed output. Native
source Z never enters the process. Source LAS must declare a usable horizontal
reference; an undeclared or unhandled compound reference is rejected.

Temporary file streams avoid pipe deadlocks. Each process checks the Target
cancellation marker and application shutdown, times out after 30 seconds and
is killed/reaped on failure. PROJ network fetching is disabled. Output size,
line count and numeric validity are checked before the batch is used.
