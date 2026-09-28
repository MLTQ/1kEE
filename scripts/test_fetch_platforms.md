# test_fetch_platforms.py

Regression tests for truthful BSEE unknown/removed status, EMODnet subsea versus
construction classification, and invalid coordinates. Run with
`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p test_fetch_platforms.py`.

BSEE identity assertion prevents collisions between repeated structure numbers in separate complexes.
