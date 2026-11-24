# wtyczka

Screen Answer: skrót klawiaturowy wysyła obraz aktywnego monitora do wybranego API AI. Odpowiedź pojawia się tylko w zasobniku systemowym — jako cyfry albo kropki na przezroczystym tle. Do własnych ćwiczeń i porównywania z własnym kluczem odpowiedzi.

Autor: [mirawer](https://github.com/mirawer).

## Pobierz Windows

**[Pobierz screen-answer.exe — Windows 10/11 x64](https://github.com/mirawer/wtyczka/releases/latest/download/screen-answer.exe)**

Albo otwórz **[Releases](https://github.com/mirawer/wtyczka/releases/latest)** → **Assets** → **screen-answer.exe**. Nie wybieraj „Source code”, jeśli chcesz gotowy program.

1. Uruchom pobrany `.exe`. Nie wymaga instalacji Rusta, Pythona, lokalnego modelu ani pakietu VC++.
2. W kreatorze wybierz dostawcę, model przyjmujący obrazy, skrót, jedną/wiele odpowiedzi i cyfry/kropki. Wklej klucz API dopiero w ostatnim, ukrytym polu (Ctrl+V lub funkcja Wklej terminala), a potem naciśnij Enter. Schowek jest odczytywany tylko na żądanie Ctrl+V, nigdy zmieniany przez aplikację. Błędne znaki powodują ponowne pytanie o klucz zamiast zamknięcia programu.
3. Po komunikacie „Działa w tle” możesz zamknąć terminal. Wyświetl całe pytanie z opcjami i naciśnij wybrany skrót.
4. Ikony mogą początkowo być w rozwijanym zasobniku Windows. Ich widoczność i kolejność kontroluje system.

Domyślny skrót to **Print Screen**. Jeżeli przejęło go Narzędzie Wycinanie lub inna aplikacja, wybierz np. **Ctrl+Shift+F8**. Program nie przestawia ustawień Windows i nie instaluje hooków. Nie uruchamiaj go jako administrator.

W PowerShellu, w katalogu z programem:

```powershell
.\screen-answer.exe status
.\screen-answer.exe stop
.\screen-answer.exe doctor
.\screen-answer.exe --selection multiple demo "1'2'3"
```

`demo` pokazuje sztuczny wynik przez 15 sekund — bez zrzutu ekranu i bez API. Program nie jest podpisany cyfrowo; system może wyświetlić ostrzeżenie. Nie wyłączaj zabezpieczeń systemowych.

## Linux — Niri + zasobnik StatusNotifierItem

Wymagane środowisko: **Niri**, **grim z obsługą JPEG**, sesyjny **D-Bus** i pasek z zasobnikiem StatusNotifierItem, np. Noctalia. W Noctalii widget `tray` musi już być włączony; program nie zmienia konfiguracji paska.

Gotowy plik x86_64 z Releases jest statycznie linkowany z musl. Po pobraniu, w jego katalogu:

```bash
chmod +x screen-answer-linux-x86_64
mkdir -p "${XDG_STATE_HOME:-$HOME/.local/state}"
./screen-answer-linux-x86_64 --runtime-dir "${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer"
# Zatrzymanie — ten sam katalog stanu:
./screen-answer-linux-x86_64 --runtime-dir "${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer" stop
```

Alternatywnie sklonuj kod i uruchom `./screen-answer`; launcher buduje release przy pierwszym starcie (wymaga Rust/Cargo 1.89+ i kompilatora C). W katalogu projektu stan jest przechowywany w `.runtime/`.

Na Linuksie skrót jest dodawany przez tymczasową nakładkę Niri, bez edycji pliku bazowego. Normalne zatrzymanie przywraca bazę. Po `kill -9` użyj `restore-keybind` z tym samym `--runtime-dir` albo uruchom program ponownie. Nie usuwaj katalogu stanu przed przywróceniem skrótu.

## Odpowiedzi i dostawcy

- Anthropic/Claude, OpenAI, OpenRouter, Gemini oraz własne API zgodne z OpenAI Chat Completions i obrazami.
- Cyfry: opcje **1–9** (A–I). Kropki: opcje **1–6** (A–F).
- Wiele odpowiedzi: np. `1'2'3`, jako pełnowymiarowe komórki, nie miniaturowy tekst w jednej ikonie.
- `-` — gotowy, `…` — analiza, `?` — brak rozpoznanej odpowiedzi, `!` — błąd.
- Jedno żądanie naraz, bez kolejkowania kolejnych naciśnięć i bez automatycznych płatnych ponowień.
- Model wybiera się numerem w kreatorze albo wpisując własny pełny identyfikator. Musi istnieć w katalogu dostawcy i być dostępny dla Twojego klucza.

### Modele w kreatorze

| Dostawca | Liczba | Wbudowane propozycje z obsługą obrazów |
| --- | ---: | --- |
| Anthropic | 5 | Haiku, Sonnet i Opus 5.5; Fable 5.2; starszy Sonnet 4.6 |
| OpenAI | 6 | GPT-5.4 mini, GPT-5.4, GPT-5.2, GPT-6 Luna, GPT-6.2 Sol i Astra |
| OpenRouter | 9 | Claude Haiku/Sonnet/Opus 5.5; GPT-5.4 mini, GPT-6 Luna, GPT-6.2 Sol; Gemini 3.8 Flash, 4 Hydrogen i 4 Carbon |
| Gemini | 8 | 3.5 Flash-Lite, 3.8 Flash, 3.6 Flash, 3.1 Flash-Lite oraz 4 Hydrogen, Argon, Neon i Carbon |

Dotychczasowe numery modeli i zapisane wybory pozostają bez zmian. Własne serwery wymagają identyfikatora z ich katalogu — nie pokazujemy im listy innego dostawcy.

Listy sprawdzono 2026-11-24 w dokumentacji [Anthropic](https://platform.claude.com/docs/en/about-claude/models/overview), [OpenAI](https://developers.openai.com/api/docs/models), [Gemini](https://aistudio.google.com/docs/models) i [publicznym katalogu OpenRouter](https://openrouter.ai/api/v1/models). To nie jest sprawdzenie dostępu dla konkretnego klucza ani test płatnego API.

**Fable 5.2 wymaga 30-dniowej retencji danych u Anthropic**, z wyjątkiem indywidualnie autoryzowanych organizacji ([zasady modelu](https://platform.claude.com/docs/en/models/fable-5-2/overview)). Kreator zaznacza to przy nazwie modelu.

## Prywatność i koszty

**Cały aktywny monitor** trafia do wybranego dostawcy, razem z widocznymi prywatnymi oknami i powiadomieniami. Sprawdź ekran przed naciśnięciem skrótu. Retencja obrazu zależy od dostawcy.

Klucz API i JPEG są obsługiwane w pamięci, nie zapisywane przez aplikację w konfiguracji, logach ani historii zrzutów. Nie wklejaj klucza do GitHuba, czatu lub argumentów komendy. Jest pamiętany tylko do zakończenia procesu; dane niesekretne wracają jako domyślne propozycje kreatora. Nie jest to izolacja od programów działających z uprawnieniami tego samego użytkownika.

Wymagane są internet oraz własny dostęp API. Usługi mogą naliczać opłaty, również za tokeny myślenia. Ustaw limit kosztów u dostawcy. Subskrypcja aplikacji czatu nie zastępuje dostępu API. Model może podać błędną odpowiedź — sprawdzamy format, nie prawdziwość rozwiązania.

## Budowanie i testy

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

GitHub Actions buduje Linux x86_64/musl i Windows x86_64/MSVC ze statycznym CRT oraz uruchamia testy jednostkowe, lokalne testy HTTP i natywne testy IPC Windows. Sprawdza też brak dodatkowych zależności DLL w Windows. Nie używa kluczy użytkownika ani płatnego API.

Testy integracyjne Linuksa: `python3 tests/smoke.py` — wymagają aktywnego sesyjnego D-Bus z zasobnikiem; używają sztucznych `niri`/`grim` i lokalnego HTTP, bez rzeczywistych zrzutów. Można wskazać binarkę przez `SCREEN_ANSWER_BIN`.

Ręczny pokaz zasobnika Windows, po pobraniu `screen-answer.exe` do katalogu projektu:

```powershell
.\scripts\test-windows.ps1 -Executable .\screen-answer.exe -ShowTray
```

Sama aplikacja nie potrzebuje skryptu. Testy jednostkowe i IPC pozostają w kodzie/CI (`cargo test`), nie są osobną aplikacją w Releases.

**Ograniczenie weryfikacji:** testy CI nie potwierdzają wyglądu zasobnika, fizycznego skrótu i przechwytywania monitora na interaktywnym Windows 10/11. Nie potwierdzają też trafności ani dostępności rzeczywistych usług AI.
