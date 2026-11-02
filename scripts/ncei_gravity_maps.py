"""Classify NCEI's named numeric products using their small format headers."""
import collections
import pathlib
import re
from urllib.parse import urlsplit

PREFIX = '/mgg/gravity/1999/data/'
ALIASES = {
    'bolivia': 'bolivia/boliviag', 'dgrav': 'dgrav/dgrav', 'dnaggrav': 'dnaggrav/dnaggrav',
    'id_boug': 'eros/id_bath/id_boug', 'id_iso': 'eros/id_bath/id_iso', 'id_ter': 'eros/id_bath/id_ter',
    'idaho': 'eros/idaho/idahog', 'maine': 'eros/maine/maine', 'nevboug': 'eros/nevada/nevboug',
    'neviso': 'eros/nevada/neviso', 'ohio': 'eros/ohio/ohiog', 'utah': 'eros/utah/utahg',
}


def canonical(key):
    if key.startswith(PREFIX + 'grids/idrisi/'):
        name = pathlib.PurePosixPath(key).name
        return PREFIX + 'grids/' + ALIASES.get(name, name + '/' + name)
    return key


def title_and_type(text):
    title = re.search(r'^file[ _]title\s*[=:]\s*(.*)', text, re.M | re.I)
    kind = re.search(r'^data[ _]type\s*[=:]\s*(.*)', text, re.M | re.I)
    return (title[1].strip() if title else '', kind[1].strip().lower() if kind else '')


def category(key, title, kind):
    value = title.lower()
    if kind == 'point':
        return 'observation_tables'
    if '/2008/data/' in key:
        return 'gravity_anomaly_maps' if pathlib.PurePosixPath(key).name.startswith('grav_down') else 'gravity_derived_roughness_maps'
    if '/global/egm96/' in key:
        return 'model_input_tables'
    if '/coeffic/' in key:
        return 'model_coefficients'
    if 'gravity data per' in value:
        return 'sampling_density_maps'
    if 'terrac' in value:
        return 'gravity_derived_terraced_maps'
    if re.search(r'depth|topograph|geology|sea surface height', value):
        return 'terrain_or_other_reference_maps'
    if re.search(r'gravity|grav anomaly|bouguer|free.air|isostatic', value):
        return 'gravity_anomaly_maps'
    if re.search(r'geoid|deflection', value):
        return 'geoid_or_deflection_maps'
    return 'unclassified'


def preferred_rank(item):
    path = pathlib.PurePosixPath(urlsplit(item['url']).path)
    if path.suffix.lower() in {'.gz', '.z', '.zip'}:
        path = path.with_suffix('')
    return ({'.bin': 0, '.nc': 1, '.grd': 2, '.img': 3, '.dat': 4,
             '.ast': 5, '.asc': 5, '.xyz': 6}.get(path.suffix.lower(), 9), item['url'])


def map_summary(files, descriptions):
    metadata = {}
    for doc in descriptions:
        if 'text' not in doc:
            continue
        key = str(pathlib.PurePosixPath(urlsplit(doc['url']).path).with_suffix(''))
        title, kind = title_and_type(doc['text'])
        if title:
            priority = 0 if doc['url'].endswith('.hdr') else 1
            if key not in metadata or priority < metadata[key][0]:
                metadata[key] = (priority, title, kind, doc['url'])
    groups = collections.defaultdict(list)
    for item in files:
        if item['numeric_key']:
            groups[canonical(item['numeric_key'])].append(item)
    products, totals = [], {}
    for key, items in sorted(groups.items()):
        info = metadata.get(key)
        if info is None:
            info = next((metadata[i['numeric_key']] for i in items if i['numeric_key'] in metadata), None)
        title, kind, source = (info[1:] if info else ('', '', None))
        if not title and '/2008/data/' in key:
            basename = pathlib.PurePosixPath(key).name
            for doc in descriptions:
                match = re.search(r'^' + re.escape(basename) + r'\.(?:img|nc|xyz)\s+([^\r\n]+)',
                                  doc.get('text', ''), re.M)
                if match:
                    title, kind, source = match[1].strip(), 'image', doc['url']
                    break
        classification = category(key, title, kind)
        sized = [i for i in items if i.get('bytes') is not None]
        chosen = min(sized, key=preferred_rank) if sized else None
        products.append({'key': key, 'title': title, 'data_type': kind,
                         'category': classification, 'description_url': source,
                         'numeric_files': len(items), 'all_numeric_formats_bytes': sum(i.get('bytes') or 0 for i in items),
                         'preferred_url': chosen['url'] if chosen else None,
                         'preferred_bytes': chosen['bytes'] if chosen else None,
                         'alternates': [i['url'] for i in items if i is not chosen]})
        t = totals.setdefault(classification, {'products': 0, 'numeric_files': 0,
                              'all_formats_bytes': 0, 'preferred_format_bytes': 0})
        t['products'] += 1
        t['numeric_files'] += len(items)
        t['all_formats_bytes'] += sum(i.get('bytes') or 0 for i in items)
        t['preferred_format_bytes'] += chosen['bytes'] if chosen else 0
    return {'categories': totals, 'products': products,
            'selection_policy': 'Merge explicitly identified IDRISI aliases; prefer canonical binary, then geographic NetCDF, then GMT grid. Choices compare storage, not scientific equivalence across projections/resolutions.',
            'placeholder_sized_numeric_files': [i['url'] for i in files if i['numeric_key'] and i.get('bytes') is not None and i['bytes'] < 100]}
