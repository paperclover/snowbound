import bpy
from pathlib import Path

root=Path(bpy.data.filepath).parent.parent
s=bpy.context.scene
stem='SnowLeopard' if s.name.startswith('2010') else 'Sequoia'
s.render.image_settings.file_format='OPEN_EXR'
s.render.image_settings.color_depth='16'
s.render.image_settings.exr_codec='ZIP'
(root/'renders').mkdir(exist_ok=True)
s.render.filepath=str(root/'renders'/f'{stem}.exr')
bpy.ops.render.render(write_still=True)
s.render.image_settings.file_format='PNG'
s.render.filepath=str(root/'renders'/f'{stem}.png')
bpy.data.images['Render Result'].save_render(s.render.filepath,scene=s)

