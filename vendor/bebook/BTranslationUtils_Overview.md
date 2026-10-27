<!-- System Overview - The Translation Kit - BTranslationUtils -->

## BTranslationUtils

[`BTranslationUtils`](BTranslationUtils.md "BTranslationUtils") contains a set of static functions which load bitmap images. The functions use the Translation Kit to convert the data from its native format to a [`BBitmap`](BBitmap.md "BBitmap") via a [`BBitmapStream`](BBitmapStream.md "BBitmapStream"), the upshot of which is that if you can convert the data to a translator bitmap, you can successfully use these functions. The image may come from a file, resource, or [`BPositionIO`](BPositionIO.md "BPositionIO"). The application is always responsible for deallocating the returned [`BBitmap`](BBitmap.md "BBitmap").
