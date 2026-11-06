# test_inventory_ncei_gravity.py

Exercises real-shaped Drupal/Apache HTML, parent/sort-link exclusion, binary
size units, numeric format grouping, compressed variants, metadata exclusion
from numeric totals and unknown size accounting. No network or payload files.
HTTP fixtures test HEAD-only requests, redirect size selection and cache reuse.
Header fixtures separate point/density/depth products from anomalies, preserve
free-air maps mentioning GEOID96, and avoid counting IDRISI mirrors or selecting
placeholder-sized ASCII exports instead of the original binary map.

Run `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_inventory_ncei_gravity.py'`.
