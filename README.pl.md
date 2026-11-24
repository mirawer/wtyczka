# Screen Answer

[![Build](https://github.com/mirawer/wtyczka/actions/workflows/build.yml/badge.svg)](https://github.com/mirawer/wtyczka/actions/workflows/build.yml)
[![Najnowsze wydanie](https://img.shields.io/github/v/release/mirawer/wtyczka)](https://github.com/mirawer/wtyczka/releases/latest)
[![Licencja: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English](README.md) · [Historia zmian](CHANGELOG.md) · [Bezpieczeństwo](SECURITY.md)

Skrót klawiaturowy wysyła obraz **całego aktywnego monitora** do wybranego API AI. Odpowiedź pojawia się tylko w zasobniku systemowym — jako cyfry albo kropki na przezroczystym tle. Do własnych ćwiczeń i porównywania z własnym kluczem odpowiedzi.

Autor: [mirawer](https://github.com/mirawer).

> **Uwaga o wersji:** instrukcje dotyczą **v0.1.5 i nowszych**. Starsze pliki do pobrania przy każdym starcie uruchamiają kreator i pytają o klucz API, przechowując go wyłącznie w pamięci; nie obsługują `--change`, `forget-key` ani `--session-only`.

## Pobieranie i obsługiwane systemy

- **[Windows 10/11 x64 — screen-answer.exe](https://github.com/mirawer/wtyczka/releases/latest/download/screen-answer.exe)**
- **[Linux x64 — screen-answer-linux-x86_64](https://github.com/mirawer/wtyczka/releases/latest/download/screen-answer-linux-x86_64)**
- [Najnowsze wydanie i wszystkie pliki](https://github.com/mirawer/wtyczka/releases/latest) — wybierz plik wykonywalny, nie „Source code”, jeśli chcesz gotowy program. `SHA256SUMS.txt` i `THIRD_PARTY_NOTICES.txt` zawierają sumy kontrolne oraz informacje o zależnościach.

| System | Obsługiwane środowisko |
| --- | --- |
| Windows | Natywny Windows 10/11 x64; bez instalacji Rusta, Pythona, lokalnego modelu i VC++ Redistributable |
| Linux | x64, **Niri**, **grim z obsługą JPEG**, sesyjny **D-Bus** i dowolny **zasobnik StatusNotifierItem**; Noctalia jest opcjonalna |
| Inne pulpity Linux / macOS | Jeszcze nieobsługiwane |

Zapisywanie klucza na Linuksie wymaga również **dostawcy Secret Service** z istniejącą, odblokowaną kolekcją domyślną (np. zgodnego systemowego magazynu kluczy). Aplikacja nie odblokowuje jej ani nie tworzy kolekcji. Secret Service używa szyfrowanego transportu sesji DH; ochrona danych na dysku zależy od dostawcy magazynu i jego polityki haseł. Jeśli magazyn jest niedostępny lub zablokowany, użyj jawnie `--session-only`; nie ma zastępczego zapisu jawnym tekstem.

Program Windows nie jest podpisany cyfrowo; system może wyświetlić ostrzeżenie. Nie wyłączaj zabezpieczeń ani nie uruchamiaj programu jako administrator. **Przed podmianą pliku EXE zatrzymaj poprzedni proces:**

```powershell
.\screen-answer.exe stop
```

## Szybki start

### Windows

1. Pobierz i uruchom `screen-answer.exe`.
2. Przy pierwszej konfiguracji wybierz dostawcę, model przyjmujący obrazy, skrót analizy, opcjonalny skrót pauzy, jedną/wiele odpowiedzi i cyfry/kropki. Wklej klucz API do **ostatniego, ukrytego pola**, a potem naciśnij Enter. Na Windows Ctrl+V jawnie odczytuje schowek; aplikacja nigdy go nie zmienia. Błędne znaki powodują ponowne pytanie o klucz zamiast zamknięcia programu.
3. Po komunikacie „Działa w tle” możesz zamknąć terminal. Wyświetl całe pytanie z opcjami i naciśnij skrót analizy.
4. Windows może początkowo umieścić ikony w rozwijanym zasobniku. Ich widoczność i kolejność kontroluje system.

Domyślny skrót analizy to **Print Screen**. Jeśli przejęło go Narzędzie Wycinanie lub inna aplikacja, wybierz np. **Ctrl+Shift+F8**. Program nie zmienia ustawień Windows ani nie instaluje hooków klawiatury. Windows sprawdza oba skróty przed pytaniem o klucz.

### Linux

Najpierw włącz zasobnik StatusNotifierItem na swoim pasku. W Noctalii widget `tray` musi już być włączony; program nie konfiguruje paska.

Gotowy plik Linuksa jest statycznie linkowany z musl; powyższe wymagania środowiska nadal obowiązują. Poza repozytorium automatycznie używa `${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer`, więc zwykły start nie potrzebuje flag:

```bash
chmod +x screen-answer-linux-x86_64
./screen-answer-linux-x86_64
```

Jeśli wybierzesz własny katalog stanu, używaj **tego samego katalogu we wszystkich poleceniach**:

```bash
chmod +x screen-answer-linux-x86_64
mkdir -p "${XDG_STATE_HOME:-$HOME/.local/state}"
./screen-answer-linux-x86_64 --runtime-dir "${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer"
# Zatrzymanie:
./screen-answer-linux-x86_64 --runtime-dir "${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer" stop
```

Albo z katalogu sklonowanego repozytorium:

```bash
./screen-answer
./screen-answer stop
```

Launcher buduje release przy pierwszym użyciu (wymaga Rust/Cargo 1.89+ i kompilatora C). W repozytorium stan trafia do `.runtime/`.

Skróty Linuksa korzystają z jednej tymczasowej nakładki Niri, bez edycji konfiguracji bazowej. Normalne zatrzymanie przywraca bazę i zwalnia oba skróty. Po `kill -9` użyj `restore-keybind` z tym samym katalogiem stanu albo uruchom program ponownie. **Nie usuwaj katalogu stanu przed przywróceniem skrótu.** `--niri-config` pozostaje flagą uruchomienia (lub jest wykrywany z Niri); nie jest zapisywanym ustawieniem.

## Zapisywanie ustawień i klucza

Pierwszy start, stare ustawienia lub ustawienia bez odwołania do klucza otwierają pełny kreator, z ukrytym polem klucza na końcu. Przy zwykłym starcie, jeśli odwołanie istnieje, ale brakuje wpisu w magazynie, konfigurację skopiowano do innego katalogu stanu albo zmieniono jej adres API, aplikacja **kończy się bezpiecznym błędem** i wskazuje `--change`, zamiast po cichu otwierać kreator lub użyć innego klucza. Po poprawnym uruchomieniu interfejsu ustawienia niesekretne i odwołanie do klucza są zapisywane atomowo. Nieudana konfiguracja zachowuje poprzednie zapisane ustawienia. Zapisywane są: dostawca, model, bazowy URL API, oba skróty, tryb odpowiedzi/wyświetlania i limit czasu.

- **Zwykły start:** z poprawnymi ustawieniami i zapisanym kluczem uruchamia program bez kreatora i pytania o klucz. Jeśli program już działa, tylko to zgłasza; bez pytań.
- **`--change`:** otwiera pełny kreator z zapisanymi wartościami jako domyślnymi. Jeśli program działa, kreator kończy się przed zatrzymaniem starego procesu i uruchomieniem nowego; to nie jest przeładowanie na żywo. Flagi nadpisujące konfigurację również korzystają z tej ścieżki ponownej konfiguracji.
- **`forget-key`:** zatrzymuje bieżący proces, wyłącza automatyczne ponowne użycie klucza i usuwa bieżące **oraz wycofane** klucze aplikacji wskazane dla tego katalogu stanu. Ustawienia niesekretne zostają. Jeśli usuwanie się nie uda (np. magazyn jest zablokowany), niesekretne odwołania do sprzątnięcia pozostają, a polecenie zwraca błąd: odblokuj magazyn i ponów je. Nie unieważnia klucza u dostawcy ani nie kasuje niezwiązanych wpisów.
- **`--session-only`:** jawnie przechowuje klucz API w RAM i **nigdy nie korzysta z magazynu kluczy**, także przy oczekującym sprzątaniu. Działa z zablokowanym magazynem oraz tymczasowym modelem/skrótem, pozostawiając istniejący zapisany klucz i konfigurację bez zmian. Jeśli zapisany klucz nie jest skonfigurowany, może zapamiętać niesekretne wybory jak w starszych wersjach. Jeśli zapisana konfiguracja już istniała, kolejny zwykły start znów z niej korzysta. **Nie** usuwa zapisanego klucza; służy do tego `forget-key`.

```powershell
# Windows, w katalogu programu (v0.1.5):
.\screen-answer.exe
.\screen-answer.exe --change
.\screen-answer.exe forget-key
.\screen-answer.exe --session-only
```

```bash
# Linux, sklonowane repozytorium (v0.1.5):
./screen-answer
./screen-answer --change
./screen-answer forget-key
./screen-answer --session-only
# Gotowy plik: użyj pobranej nazwy i tego samego --runtime-dir co wyżej.
```

Klucze są zapisywane przez **Menedżera poświadczeń Windows** albo **Secret Service Linuksa**, nie w `settings.json`. **Nie ma automatycznego zapisu zastępczego jawnym tekstem.** Zapis klucza na Windows przyjmuje maksymalnie **2560 bajtów ASCII**; wprowadzanie w trybie session-only — **4096 bajtów ASCII**. Klucze są powiązane z użytkownikiem systemu, kanonicznym katalogiem stanu, dostawcą i bazowym URL API oraz losowym identyfikatorem wpisu. Przeniesienie konfiguracji lub zmiana jej adresu docelowego nie może ponownie użyć klucza z innego zakresu. Nie jest to izolacja od innych programów tego samego użytkownika.

**Przed przeniesieniem lub usunięciem katalogu stanu wykonaj skutecznie `forget-key`.** Nieudane usunięcia wycofanych kluczy po rotacji są śledzone przez niesekretne odwołania do sprzątnięcia i ponawiane przy zwykłym starcie; tryb session-only nie dotyka tych odwołań ani magazynu. Sprzątanie zasobów systemowych po nagłej awarii jest wykonywane w miarę możliwości; aplikacja nie gwarantuje wymazania każdej kopii pamięci systemu i bibliotek.

## Sterowanie i zasobnik

```powershell
.\screen-answer.exe status
.\screen-answer.exe toggle-pause
.\screen-answer.exe stop
.\screen-answer.exe doctor
# Wyłącznie sztuczny pokaz zasobnika; bez zrzutu, klucza i zapytania API:
.\screen-answer.exe --selection multiple demo "1'2'3"
```

Na Linuksie użyj `./screen-answer` w repozytorium albo gotowego pliku z jego katalogiem stanu. `demo` pokazuje sztuczny wynik przez 15 sekund; nie jest testem AI ani przechwytywania. `doctor` sprawdza platformę/konfigurację bez zrzutu i zapytania API. `status` podaje stan procesu, nigdy odpowiedź modelu.

Ustaw opcjonalny skrót pauzy, np. **Ctrl+Shift+F9**, w kreatorze albo przez `--pause-keybind Ctrl+Shift+F9`. Domyślnie brak. Enter zachowuje zapisany wybór; `brak` lub `off` w kreatorze (albo `--pause-keybind off`) usuwa go. Musi być inny niż skrót analizy.

- Pauza przerywa lokalne oczekiwanie i czyści wszystkie komórki do zwykłego **`-`**, identycznego jak po starcie. Bez specjalnej ikony pauzy, tekstu po najechaniu, dymka i powiadomienia.
- W pauzie skrót analizy niczego nie przechwytuje ani nie wysyła. Wznowienie nadal pokazuje `-`; dopiero nowe naciśnięcie skrótu analizy zaczyna kolejne pytanie. Stare ani spóźnione wyniki nie wracają.
- Klucz pozostaje dostępny dla działającego procesu. `stop` kończy go całkowicie; nowy start jest aktywny, nie w pauzie.
- Żądania odebranego już przez dostawcę nie można cofnąć; może nadal zostać naliczona opłata. Spóźniona odpowiedź jest odrzucana.

### Odpowiedzi i dostawcy

- Anthropic/Claude, OpenAI, OpenRouter, Gemini oraz własne API zgodne z OpenAI Chat Completions **i obrazami**.
- Cyfry: **1–9** (A–I). Kropki: **1–6** (A–F). Wiele odpowiedzi, np. `1'2'3`, zajmuje pełnowymiarowe komórki, a nie miniaturowy tekst w jednej ikonie.
- `-`: gotowy lub wstrzymany (ten sam wygląd); `…`: analiza; `?`: brak rozpoznanej odpowiedzi; `!`: błąd.
- Jedno żądanie naraz; kolejne naciśnięcia nie są kolejkowane, bez automatycznych płatnych ponowień.

### 28 wbudowanych propozycji modeli

| Dostawca | Liczba | Propozycje z obsługą obrazów |
| --- | ---: | --- |
| Anthropic | 5 | Haiku, Sonnet i Opus 5.5; Fable 5.2; starszy Sonnet 4.6 |
| OpenAI | 6 | GPT-5.4 mini, GPT-5.4, GPT-5.2, GPT-6 Luna, GPT-6.2 Sol i Astra |
| OpenRouter | 9 | Claude Haiku/Sonnet/Opus 5.5; GPT-5.4 mini, GPT-6 Luna, GPT-6.2 Sol; Gemini 3.8 Flash, 4 Hydrogen i 4 Carbon |
| Gemini | 8 | 3.5 Flash-Lite, 3.8 Flash, 3.6 Flash, 3.1 Flash-Lite; 4 Hydrogen, Argon, Neon i Carbon |

Wybierz numer albo wpisz własny pełny identyfikator modelu. Dotychczasowe numery i zapisane wybory pozostają bez zmian. Własne serwery wymagają identyfikatora z ich katalogu, nie listy innego dostawcy. Model musi istnieć i być dostępny dla Twojego klucza API.

Listy sprawdzono 2026-11-24 w dokumentacji [Anthropic](https://platform.claude.com/docs/en/about-claude/models/overview), [OpenAI](https://developers.openai.com/api/docs/models), [Gemini](https://aistudio.google.com/docs/models) i [publicznym katalogu OpenRouter](https://openrouter.ai/api/v1/models). Nie sprawdza to dostępu dla Twojego klucza i nie jest testem płatnego API.

**Fable 5.2 wymaga 30-dniowej retencji danych u Anthropic**, z wyjątkiem indywidualnie autoryzowanych organizacji ([zasady modelu](https://platform.claude.com/docs/en/models/fable-5-2/overview)). Kreator zaznacza to przy nazwie modelu.

## Prywatność i koszty

**Cały aktywny monitor** trafia do wybranego API, razem z widocznymi prywatnymi oknami i powiadomieniami. Sprawdź ekran przed naciśnięciem skrótu. Retencja obrazów i pytań zależy od dostawcy; własny endpoint otrzymuje te same wrażliwe treści.

Program obsługuje zrzuty/JPEG w pamięci, bez zapisywania historii zrzutów. Nie zapisuje kluczy API w konfiguracji, logach ani historii poleceń. Od v0.1.5 klucze można zapisywać w systemowym magazynie, jak opisano wyżej; `--session-only` przechowuje klucz sesji w RAM bez zmieniania istniejącej zapisanej konfiguracji. Nie umieszczaj klucza w argumentach polecenia, na GitHubie, w czacie ani zgłoszeniach błędów. Nigdy nie publikuj prywatnych zrzutów ani pytań; zobacz [SECURITY.md](SECURITY.md).

Wymagane są internet i własny dostęp API. Dostawcy mogą naliczać opłaty, także za tokeny myślenia. Ustaw limit kosztów u dostawcy. Subskrypcja aplikacji czatu nie zastępuje dostępu API. Modele mogą się mylić: aplikacja sprawdza format odpowiedzi, nie jej prawdziwość.

## Rozwój i ograniczenia weryfikacji

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

GitHub Actions buduje Linux x86_64/musl i Windows x86_64/MSVC ze statycznym CRT, uruchamia testy jednostkowe/lokalne HTTP i natywne testy IPC Windows oraz sprawdza brak dodatkowych zależności DLL Windows. Te sprawdzenia nie używają kluczy użytkownika ani płatnego API.

Testy integracyjne Linuksa: `python3 tests/smoke.py`. Wymagają aktywnego sesyjnego D-Bus z zasobnikiem i używają sztucznych `niri`/`grim` oraz lokalnego HTTP, nie rzeczywistych zrzutów. Ustaw `SCREEN_ANSWER_BIN`, aby wybrać konkretną binarkę.

Testy zapisywania kluczy Linuksa: najpierw `cargo test --locked`, aby skompilować narzędzia testowe, potem `python3 tests/persistence.py`. Wymagają `dbus-daemon` i `gnome-keyring-daemon`, używają prywatnej magistrali i jednorazowego magazynu, nigdy nie korzystają z wpisów użytkownika. Używają sztucznych kluczy/obrazów i lokalnego HTTP, nie płatnych API. `SCREEN_ANSWER_BIN` pozwala wybrać plik programu.

Opcjonalny pokaz zasobnika Windows, po pobraniu pliku programu do repozytorium:

```powershell
.\scripts\test-windows.ps1 -Executable .\screen-answer.exe -ShowTray
```

Sama aplikacja nie potrzebuje skryptu. Testy jednostkowe/IPC są w kodzie i CI (`cargo test`), a nie jako osobna aplikacja w bieżących plikach wydania.

**Ograniczenia weryfikacji:** CI nie potwierdza wyglądu zasobnika, fizycznych skrótów ani przechwytywania monitora na interaktywnym Windows 10/11. Nie potwierdza też dostępności rzeczywistych dostawców ani trafności odpowiedzi. Historię wersji opisuje [CHANGELOG.md](CHANGELOG.md); nie podajemy tu liczby testów dla poszczególnych platform.

## Licencja

[MIT](LICENSE). Copyright © 2026 mirawer. Uruchom `--license`, aby wyświetlić pełny tekst licencji.
