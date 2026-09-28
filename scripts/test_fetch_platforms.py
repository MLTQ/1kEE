import unittest
from fetch_platforms import normalize

class NormalizationTests(unittest.TestCase):
    def feature(self, p, xy=None):
        return dict(id='platforms.1', geometry=dict(type='Point', coordinates=xy or [4.0, 56.0]), properties=p)

    def test_missing_removal_does_not_imply_active(self):
        p = normalize('bsee', self.feature(dict(STRUCTURE_=1, COMPLEX_ID=2, STRUCTURE1='A')))
        self.assertEqual(p['source_id'], '2/1')
        self.assertFalse(p['historical'])
        self.assertIn('unknown', p['status'])
        p = normalize('bsee', self.feature(dict(STRUCTURE_=1, COMPLEX_ID=2, REMOVAL_DA='2000-01-01')))
        self.assertTrue(p['historical'])
        self.assertEqual(p['removed'], '2000-01-01')

    def test_status_and_subsea_are_independent(self):
        p = normalize('emodnet', self.feature(dict(current_status='Operational', category='Subsea steel')))
        self.assertTrue(p['support'])
        self.assertFalse(p['historical'])
        p = normalize('emodnet', self.feature(dict(current_status='Under construction', category='Fixed steel')))
        self.assertFalse(p['support'])
        self.assertTrue(p['planned'])
        with self.assertRaises(ValueError):
            normalize('emodnet', self.feature({}, [10, 120]))

if __name__ == '__main__':
    unittest.main()
