local fu = Fusion()
local root = assert(arg[1], 'Pass the absolute design directory')
local path = root .. '/sources/Snowbound-Finishing.comp'
local comp
for _, c in pairs(fu:GetCompList()) do
 if c:GetAttrs().COMPS_FileName == path then comp = c end
end
comp = comp or fu:LoadComp(path)
for name,file in pairs({Notebook_2010='SnowLeopard',Notebook_15='Sequoia'}) do
 comp:FindTool(name).Clip = root .. '/renders/' .. file .. '.png'
end
print('Render',comp:Render({Start=0,End=0,Wait=true}))
for name,file in pairs({Notebook_2010='SnowLeopard',Notebook_15='Sequoia'}) do
 comp:FindTool(name).Clip = 'Comp:/../renders/' .. file .. '.png'
end
print('Save',comp:Save())

for _,name in ipairs({'SnowLeopard','Sequoia'}) do
 local base = root .. '/Snowbound-' .. name
 assert(os.rename(base .. '0000.png', base .. '.png'))
end
