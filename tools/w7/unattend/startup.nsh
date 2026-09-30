@echo -off
# edk2 on arm64 lists no boot option for the USB installer disc, so its shell starts it.
for %m in fs0 fs1 fs2 fs3 fs4 fs5
  if exist %m:\efi\boot\bootaa64.efi then
    %m:\efi\boot\bootaa64.efi
  endif
endfor
