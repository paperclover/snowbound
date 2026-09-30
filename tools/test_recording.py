from pathlib import Path
import hashlib
import json
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns, texts

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/recording'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def recorded(read):
    """The page holding a recording, its playlist, and each paragraph's text or file with
    the moment it links to and the recording it holds."""
    page = next(page for page in (ET.parse(path).getroot() for path in sorted(read.glob('page-*.xml')))
                if page.find('one:MediaPlaylist', ns) is not None)
    playlist = [reference.get('mediaID') for reference in
                page.findall('one:MediaPlaylist/one:MediaReference', ns)]
    links = []
    for element in page.findall('.//one:OE', ns):
        index = element.find('one:MediaIndex', ns)
        media = element.find('one:MediaFile', ns)
        text = element.find('one:T', ns)
        shown = '[%s]' % media.get('preferredName') if media is not None else texts(element)[0] if text is not None else ''
        link = None
        if index is not None:
            link = (index.find('one:MediaReference', ns).get('mediaID'), int(index.get('timeIndex')))
        links.append((shown, link, media.find('one:MediaReference', ns).get('mediaID') if media is not None else None))
    return page, playlist, links


def payload(read, name):
    payloads = json.loads((read / 'payloads.json').read_text(encoding='utf-8-sig'))
    entry, = (entry for entry in payloads if entry['name'] == name)
    data = (read / (entry['sha256'] + '.attachment')).read_bytes()
    assert hashlib.sha256(data).hexdigest() == entry['sha256']
    return entry, data


class RecordingTest(unittest.TestCase):
    def cold(self, row):
        read = FIXTURE / row / 'cold/read'
        with TemporaryDirectory() as temporary:
            copy = Path(temporary) / 'read'
            shutil.copytree(read, copy)
            compare((FIXTURE / row / 'candidate').resolve(), copy)
        return read

    def test_snowbound_s_recording_opens_in_onenote_with_its_line_links_and_audio(self):
        read = self.cold('.')
        _, playlist, links = recorded(read)
        recording, = playlist
        shown = [text for text, _, _ in links]
        self.assertEqual(shown, ['Before recording', '[Recorded.wav]', '',
                                 'Audio recording started: 5:18 PM Tuesday, September 29, 2026',
                                 'First linked note', 'Second linked note'])
        self.assertEqual(links[1][2], recording)
        self.assertEqual([link for _, link, _ in links[1:4]], [(recording, 0), None, (recording, 0)])
        first, second = (link[1] for _, link, _ in links[4:])
        self.assertTrue(40 <= first < second)
        _, data = payload(read, 'Recorded.wav')
        # 4-bit IMA ADPCM (WAVE_FORMAT_DVI_ADPCM), mono 16 kHz, which OneNote plays
        # (`played-from-note.png`).
        self.assertEqual(data[:4] + data[8:16], b'RIFFWAVEfmt ')
        self.assertEqual(int.from_bytes(data[20:22], 'little'), 0x11)
        self.assertEqual(int.from_bytes(data[24:28], 'little'), 16000)

    def test_snowbound_s_video_recording_opens_in_onenote_as_a_video_recording(self):
        read = self.cold('video')
        _, playlist, links = recorded(read)
        recording, = playlist
        shown = [text for text, _, _ in links]
        self.assertEqual(shown, ['Before recording', '[Recorded.avi]', '',
                                 'Video recording started: 5:18 PM Tuesday, September 29, 2026',
                                 'First linked note', 'Second linked note'])
        self.assertEqual(links[1][2], recording)
        first, second = (link[1] for _, link, _ in links[4:])
        self.assertTrue(40 <= first < second)
        entry, data = payload(read, 'Recorded.avi')
        self.assertEqual(entry['kind'], 'MediaFile')
        # Motion JPEG and PCM in AVI, which OneNote plays (`video/played-from-note.png`).
        self.assertEqual(data[:4] + data[8:12], b'RIFFAVI ')
        self.assertIn(b'vidsMJPG', data[:1024])
        self.assertIn(b'auds', data[:1024])

    def test_onenote_lists_avi_mpg_and_wmv_files_as_recordings_but_not_mp4_or_mov(self):
        page = ET.parse(FIXTURE / 'native/read/attached-video.xml').getroot()
        kinds = {element.get('preferredName'): element.tag.split('}')[1]
                 for element in page.iter()
                 if element.tag.endswith(('}MediaFile', '}InsertedFile'))}
        self.assertEqual(kinds['mjpeg-pcm.avi'], 'MediaFile')
        self.assertEqual(kinds['mpeg1.mpg'], 'MediaFile')
        self.assertEqual(kinds['wmv2.wmv'], 'MediaFile')
        self.assertEqual(kinds['h264-aac.mp4'], 'InsertedFile')
        self.assertEqual(kinds['h264-aac.mov'], 'InsertedFile')

    def test_notes_written_while_onenote_s_recording_is_paused_are_not_linked(self):
        page = ET.parse(FIXTURE / 'native/read/paused-while-recording.xml').getroot()
        linked = {texts(element)[0]: element.find('one:MediaIndex', ns) is not None
                  for element in page.findall('.//one:OE', ns)
                  if element.find('one:T', ns) is not None and texts(element)[0]}
        self.assertTrue(linked['Another note here'])
        self.assertFalse(linked['Typed while paused'])
        self.assertTrue(linked['After resuming'])

    def test_onenote_s_recording_opens_as_recorded_after_snowbound_deletes_restores_and_links(self):
        read = self.cold('edit')
        _, playlist, links = recorded(read)
        native = FIXTURE / 'native/read'
        _, native_playlist, native_links = recorded(native)
        self.assertEqual(playlist, native_playlist)
        self.assertEqual(links[:len(native_links)], native_links)
        recording, = playlist
        self.assertEqual(links[len(native_links):], [('Linked by Snowbound', (recording, 25000), None)])
        entry, data = payload(read, 'Recorded.wma')
        self.assertEqual(entry['kind'], 'MediaFile')
        self.assertTrue(data.startswith(bytes.fromhex('3026b2758e66cf11')))


if __name__ == '__main__':
    unittest.main()
