@echo off
setlocal

cargo build --release
if errorlevel 1 exit /b 1

if not exist dist mkdir dist
copy /y target\release\stitch.exe dist\stitch.exe

echo.
echo Build output in dist\stitch.exe
