# test_fetch_pipelines.py

Offline regression fixtures reproduce the real GEM three-ordinate/multipart
geometry issue, verify ownership is not mislabeled as operation, and distinguish
BSEE fuel/history records from non-fuel umbilicals. Run with
`python3 -m unittest discover -s scripts -p 'test_fetch_pipelines.py'`.
