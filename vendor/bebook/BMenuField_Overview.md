<!-- System Overview - The Interface Kit - BMenuField -->

## BMenuField  
  
A [`BMenuField`](BMenuField.md "BMenuField") object displays a labeled pop-up menu. It's a simple object that employs a [`BMenuBar`](BMenuBar.md "BMenuBar") object to control a [`BMenu`](BMenu.md "BMenu"). All it adds to what a [`BMenuBar`](BMenuBar.md "BMenuBar") can do on its own is a label and a more control-like user interface that includes keyboard navigation.

The functions defined in this class resemble those of a [`BControl`](BControl.md "BControl") ([`SetLabel()`](BMenuField.md#BMenuField_SetLabel "SetLabel\(\), Label\(\)"), [`IsEnabled()`](BMenuField.md#BMenuField_IsEnabled)), especially a BTextControl ([`SetDivider()`](BMenuField.md#BMenuField_SetDivider "SetDivider\(\), Divider\(\)"), [`Alignment()`](BMenuField.md#BMenuField_Alignment)). However, unlike a real [`BControl`](BControl.md "BControl") object, a [`BMenuField`](BMenuField.md "BMenuField") doesn't maintain a current value and it can't be invoked to send a message. All the control work is done by items in the [`BMenu`](BMenu.md "BMenu").
