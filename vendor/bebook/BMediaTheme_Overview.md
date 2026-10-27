<!-- System Overview - The Media Kit - BMediaTheme -->

## BMediaTheme

A [`BMediaTheme`](BMediaTheme.md "BMediaTheme") is an object that, given a [`BParameterWeb`](BParameterWeb.md "BParameterWeb"), can create a [`BView`](BView.md "BView") containing all the controls needed to configure the [`BControllable`](BControllable.md "BControllable") node represented by the [`BParameterWeb`](BParameterWeb.md "BParameterWeb"). You can then add the returned [`BView`](BView.md "BView") to a window so the user can configure the node to their liking.

The resulting [`BView`](BView.md "BView") contains not only the controls for configuring the parameters available in the node, but also indicates the data flow path through the node.

The BeOS includes the standard theme, which provides the system default look to media controls, as seen in the Audio and Video preference applications beginning with Release 4.

If you want to change the look of media controls within the BeOS, you can write your own [`BMediaTheme`](BMediaTheme.md "BMediaTheme") and install it as the default system theme by calling [`SetPreferredTheme()`](BMediaTheme.md#BMediaTheme_SetPreferredTheme). If you want to write a new theme and provide the ability for the user to enable it as the new system theme, you'll need to write an application that calls [`SetPreferredTheme()`](BMediaTheme.md#BMediaTheme_SetPreferredTheme).

If you want to return to the default system theme, you should call `SetPreferredTheme`(`NULL`). Make sure you don't have any theme views instantiated in your application when you do so.

* * *

### Installing a Theme Add-on

Theme add-ons belong at `/boot/home/config/add-ons/media/themes`. They can have any name you like.

Warning

![Warning](./images/admonitions/Stop_32.png)

At this time, theme add-on support has not been sufficiently tested and may or may not work. If you have problems with it, please file a bug report at http://bebugs.be.com.
