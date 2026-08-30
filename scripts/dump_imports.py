#example script

import sys
 
for name, mod in sorted(sys.modules.items()):
    if mod is None:
        continue
    path = getattr(mod, '__file__', None) or '<built-in>'
    version = getattr(mod, '__version__', '')
    version_str = f"  v{version}" if version else ''
    print(f"{name:<40} {str(path):<60}{version_str}")
    
print(f"\nTotal: {len(sys.modules)} modules")