#!/usr/bin/env python3
"""Download public offshore installation snapshots; preserve source distinctions."""
import argparse
import collections
import datetime
import hashlib
import json
import math
import pathlib
import subprocess

BSEE = 'https://www.data.bsee.gov/Mapping/Files/platform.zip'
EMODNET = 'https://ows.emodnet-humanactivities.eu/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=emodnet%3Aplatforms&outputFormat=application%2Fjson&srsName=CRS%3A84&count=20000'
METADATA = 'https://emodnet.ec.europa.eu/geonetwork/srv/api/records/ddbe3597-4e3f-4e74-8d31-947c4efef2e9/formatters/xml'


def fetch(url, path):
    if path.exists():
        return
    part = path.with_suffix(path.suffix + '.part')
    try:
        subprocess.run(['curl', '-fLsS', '--retry', '3', '--max-time', '180', url, '-o', str(part)], check=True)
        part.rename(path)
    finally:
        part.unlink(missing_ok=True)


def normalize(source, feature):
    g, p = feature['geometry'], feature['properties']
    if g['type'] != 'Point':
        raise ValueError('Expected platform point')
    lon, lat = g['coordinates'][:2]
    if not all(math.isfinite(v) for v in (lon, lat)) or abs(lon) > 180 or abs(lat) > 90:
        raise ValueError('Invalid platform position')
    def val(k):
        return str(p.get(k) or '').strip()
    item = dict(source=source, source_id='', name='', operator='', country='', product='',
                kind='', function='', status='', installed='', removed='', water_depth_m=None,
                historical=False, planned=False, support=False, lat=lat, lon=lon)
    if source == 'bsee':
        # No removal date is not a claim of current production or active operation.
        if not val('COMPLEX_ID') or not val('STRUCTURE_'):
            raise ValueError('Missing BSEE complex/structure identifier')
        item.update(source_id=val('COMPLEX_ID') + '/' + val('STRUCTURE_'),
                    name=f"Complex {val('COMPLEX_ID')} / structure {val('STRUCTURE1')}",
                    country='United States', kind='Platform structure',
                    installed=val('INSTALL_DA'), removed=val('REMOVAL_DA'),
                    status='Removed' if val('REMOVAL_DA') else 'Removal not recorded; operating status unknown',
                    historical=bool(val('REMOVAL_DA')))
    else:
        status = val('current_status')
        kind, function = val('category'), val('function')
        classification = (kind + ' ' + function).lower()
        item.update(source_id=str(feature['id']), name=val('name') or val('platformid'),
                    operator=val('operator'), country=val('country'), product=val('primary_production'),
                    kind=kind or 'Unspecified installation', function=function, status=status or 'Unknown',
                    installed=val('valid_from'), removed=val('valid_to'), water_depth_m=p.get('water_depth'),
                    historical=status.lower() in {'decommissioned', 'closed down', 'removed'},
                    planned=status.lower() in {'under construction', 'planned'},
                    support=any(s in classification for s in ['subsea', 'buoy', 'terminal']))
    if not item['source_id']:
        raise ValueError('Missing stable platform identifier')
    return item


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out-dir', required=True, type=pathlib.Path)
    root = parser.parse_args().out_dir
    root.mkdir(parents=True, exist_ok=True)
    output = root / 'platforms.json'
    if output.exists():
        raise SystemExit('Completed inventory exists; choose a new snapshot directory')
    fetch(BSEE, root / 'bsee-platform.zip')
    fetch(EMODNET, root / 'emodnet-platforms.geojson')
    fetch(METADATA, root / 'emodnet-metadata.xml')
    converted = root / 'bsee-platforms.geojson'
    if not converted.exists():
        part = root / 'bsee-wgs84.part.geojson'
        try:
            subprocess.run(['ogr2ogr', '-f', 'GeoJSON', '-t_srs', 'EPSG:4326', '-ct_opt', 'ALLOW_BALLPARK=NO',
                            '-lco', 'RFC7946=YES', str(part), '/vsizip/' + str((root / 'bsee-platform.zip').resolve())], check=True)
            part.rename(converted)
        finally:
            part.unlink(missing_ok=True)
    items = []
    for source in ['bsee', 'emodnet']:
        data = json.loads((root / f'{source}-platforms.geojson').read_text())
        if source == 'emodnet' and (data['numberMatched'] != len(data['features']) or data['numberReturned'] != len(data['features'])):
            raise ValueError('WFS response was truncated; refusing incomplete inventory')
        items.extend(normalize(source, f) for f in data['features'])
    if len({(p['source'], p['source_id']) for p in items}) != len(items):
        raise ValueError('Duplicate platform identifiers')
    manifest = dict(version=1, retrieved_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        coverage='BSEE Gulf federal waters; EMODnet European seas. Static inventory, not live rig positions.',
        sources=[dict(id='bsee', url=BSEE, attribution='BSEE, US Government data',
                      caveat='All structure records, including removals. No removal date does not confirm operation. NAD27 transformed to WGS84.'),
                 dict(id='emodnet', url=EMODNET, metadata=METADATA, attribution='EMODnet Human Activities / Cogea Srl, CC BY 4.0',
                      caveat='Harmonized inventory; source vintages vary. Includes floating and subsea installations, not a mobile-rig live feed.')],
        files={p.name: dict(bytes=p.stat().st_size, sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in root.iterdir() if p.is_file()},
        counts=dict(collections.Counter(p['source'] for p in items)),
        visible_by_default=sum(not(p['historical'] or p['planned'] or p['support']) for p in items))
    payload = dict(version=1, manifest=manifest, platforms=items)
    part = root / 'platforms.json.part'
    part.write_text(json.dumps(payload, ensure_ascii=False, separators=(',', ':'), allow_nan=False))
    part.rename(output)
    print(json.dumps({k: manifest[k] for k in ['counts', 'visible_by_default']}, indent=2))


if __name__ == '__main__':
    main()
