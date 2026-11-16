"""Regression checks for compact Lucide path syntax: python3 -m unittest discover -s tools."""
import unittest
from convert import path


class Paths(unittest.TestCase):
    def test_initial_relative_move_is_translated_once(self):
        self.assertEqual(path('m4 6 2 2m2 2'), 'm 4 5 1 1 m 1 1')

    def test_compact_arc_flags_and_endpoints(self):
        self.assertEqual(path('M0 0a3 2.5 0 016 0'), 'M 2 2 a 1.5 1.25 0 0 1 3 0')
        self.assertEqual(path('M0 0a1.5 1.5 0 00-2.474-1.561'), 'M 2 2 a 0.75 0.75 0 0 0 -1.237 -0.7805')

    def test_absolute_arc_and_close(self):
        self.assertEqual(path('M2 4A4 6 45 1 0 20 22Z'), 'M 3 4 A 2 3 45 1 0 12 13 Z')
