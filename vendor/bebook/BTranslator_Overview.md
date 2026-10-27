<!-- System Overview - The Translation Kit - BTranslator -->

## BTranslator

[`BTranslator`](BTranslator.md "BTranslator") is an abstract superclass that you subclass to define your own proprietary translator objects, one translator per [`BTranslator`](BTranslator.md "BTranslator") subclass. You add instances of your [`BTranslator`](BTranslator.md "BTranslator") subclasses to a [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") through [`BTranslatorRoster::AddTranslators()`](BTranslatorRoster.md#BTranslatorRoster_AddTranslators "AddTranslators\(\)"). [`BTranslator`](BTranslator.md "BTranslator") objects that your app creates and adds to the Roster are not visible to other applications.

Note that the [`BTranslator`](BTranslator.md "BTranslator") destructor is protected; you never delete a [`BTranslator`](BTranslator.md "BTranslator") from outside the class. Insead, you [`Release()`](BTranslator.md#BTranslator_Release) it. See [`Acquire()`](BTranslator.md#BTranslator_Acquire "Acquire\(\), Release\(\), ReferenceCount\(\)") for details.

The primary [`BTranslator`](BTranslator.md "BTranslator") functions are similar to the functions and data that a translator add-on supplies. Most of the [`BTranslator`](BTranslator.md "BTranslator") functions take you to the Translator Add-ons descriptions.
