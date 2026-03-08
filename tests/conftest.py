from pathlib import Path
import sys


CLIENT_SRC = Path(__file__).resolve().parents[1] / "aeroboard-client" / "src"
if str(CLIENT_SRC) not in sys.path:
    sys.path.insert(0, str(CLIENT_SRC))