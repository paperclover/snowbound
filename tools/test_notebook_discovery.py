"""Compare library discovery with retained native OneNote hierarchy captures."""
import json
import os
from pathlib import Path
import subprocess
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent


class NativeDiscovery(unittest.TestCase):
    def test_native_hierarchies(self):
        fixtures = ['m6/native-features-01', 'native-encrypted/cold-encrypted-02',
                    'native-delete/cold-deletion-05', 'native-external-assets']
        if os.environ.get('ONESTORE_NOTEBOOK_NATIVE'):
            fixtures.append(str(Path(os.environ['ONESTORE_NOTEBOOK_NATIVE']).resolve()))
        for fixture in fixtures:
            with self.subTest(fixture=fixture):
                source = ROOT / 'corpus' / fixture
                catalog = json.loads(subprocess.check_output(
                    [str(ROOT / 'target/debug/examples/discover'), str(source / 'notebook')]))
                hierarchy = ET.parse(source / 'read/hierarchy.xml').getroot()
                notebook = next(node for node in hierarchy.iter() if node.tag.endswith('}Notebook'))

                def path(value):
                    return '/'.join(part for part in value.replace('\\', '/').split('/') if part)

                prefix = path(notebook.get('path'))

                def native(node):
                    sections = []
                    groups = []
                    for child in node:
                        if child.tag.endswith('}Section'):
                            relative = path(child.get('path'))[len(prefix) + 1:]
                            name = child.get('name')
                            if Path(relative).parts[-2:] == ('OneNote_RecycleBin', 'OneNote_DeletedPages.one'):
                                self.assertEqual(name, 'Deleted Pages')
                                name = 'OneNote_DeletedPages'
                            sections.append((relative, name))
                        elif child.tag.endswith('}SectionGroup'):
                            groups.append(native(child))
                    return {'path': path(node.get('path'))[len(prefix) + 1:], 'sections': sections, 'groups': groups}

                def actual(folder):
                    sections = []
                    for section in folder['sections']:
                        state = section['state']
                        name = state.get('Readable', {}).get('name') if isinstance(state, dict) else None
                        sections.append((section['path'], Path(section['path']).stem if name is None else name))
                    return {'path': folder['path'], 'sections': sections,
                            'groups': [actual(group) for group in folder['groups']]}

                self.assertEqual(actual(catalog), native(notebook))
                if fixture == 'm6/native-features-01':
                    self.assertEqual(len(catalog['toc']['unresolved']), 1)
                    self.assertEqual(catalog['toc']['unresolved'][0]['filename'], 'synthetic.one')
                elif fixture == 'native-encrypted/cold-encrypted-02':
                    states = {section['path']: section['state'] for section in catalog['sections']}
                    self.assertEqual(states['synthetic.one'], 'Locked')
                    self.assertEqual(states['Open Notebook.one']['Unreadable']['message'],
                                     'Object space has no revision manifest list')


if __name__ == '__main__':
    unittest.main()
