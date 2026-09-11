# Controller double delivery hardware checks

Some controllers, or the input stack between them and Degauss, deliver one physical press as two very fast press and release pairs. Degauss drops the second pair of the same action when it arrives within 40 ms of the first, before the press reaches its own held-key repeater. The host tests cover the sequences themselves; the checks below need a real MiSTer, a keyboard, an ordinary controller and, for the last section, a controller known to deliver presses twice.

## Ordinary keyboard and controller

Use a keyboard and a controller that deliver each press once. Nothing about them may feel different.

1. Tap up and down once in the Categories home screen, a system list, a folder, Favourites, search results, Actions, Menu and every Options page. Each tap moves one row.
2. Repeat single taps of left and right with each **Left and Right Behaviour** value. Each tap changes the speed, jumps one letter, jumps one page or moves one row, once.
3. Tap A, B, X and Y once each where they apply. Each press acts once: one folder opened, one level backed out, one Actions list, one Menu.
4. Hold up and down through the initial delay at every **Scroll Speed**, including the fastest. The scroll starts after the same delay and runs at the same cadence as before. Hold left and right in the plain movement setting and confirm they scroll the same way.
5. Enable **Hold X (1s) to Add/Remove Fav** and **Hold Y (1s) for Random Game**. A one-second hold fires each shortcut once; a short press still opens Actions or Menu. Releasing the button on the screen a shortcut opened does nothing there.
6. Tap the same direction twice in quick succession, deliberately. Both taps move.

## Affected controller

Use a controller or input stack that delivers one press as two pairs.

1. Tap up and down in the same views as above. Each tap moves exactly one row.
2. Tap left and right with each **Left and Right Behaviour** value. Each tap acts once.
3. Tap A, B, X and Y. Each press acts once and no screen is opened or left twice.
4. Hold a direction. The scroll starts after the initial delay, runs at the configured speed and stops on release; nothing keeps scrolling afterwards.
5. Reverse direction quickly, and alternate left and right quickly. Every change of direction acts immediately.
6. Hold X and Y for one second with their shortcuts enabled. Each shortcut fires once, and releasing the button afterwards does nothing.
7. Connect the ordinary controller alongside the affected one and repeat the taps. One press is one action whichever controller sends it.

Record which controller and MiSTer input configuration was used and which checks were performed. A check not performed is not a pass.
