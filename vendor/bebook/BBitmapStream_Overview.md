<!-- System Overview - The Translation Kit - BBitmapStream -->

## BBitmapStream

[`BBitmapStream`](BBitmapStream.md "BBitmapStream") allows for the easy conversion of a translator bitmap, the default Translation Kit bitmap format, to a [`BBitmap`](BBitmap.md "BBitmap"). It's a very limited subclass of [`BPositionIO`](BPositionIO.md "BPositionIO") capable only of reading and writing translator bitmaps; performing i/o on the object with other types of data can yield strange results. The main attraction of the class is the [`DetachBitmap()`](BBitmapStream.md#BBitmapStream_DetachBitmap "DetachBitmap\(\)") method, which returns the contents of the object as a [`BBitmap`](BBitmap.md "BBitmap").

Most of the time, you won't need to use [`BBitmapStream`](BBitmapStream.md "BBitmapStream") directly; [`BTranslationUtils`](BTranslationUtils.md "BTranslationUtils") contains the functionality required to load images from files, resources, or general BPositionIOs into [`BBitmap`](BBitmap.md "BBitmap")s.

The following snippet illustrates typical usage of [`BBitmapStream`](BBitmapStream.md "BBitmapStream") sans proper error checking:
    
    
    BBitmap *FetchBitmap(char *filename)
    {
        BFile file(filename, B_READ_ONLY);
        BTranslatorRoster *roster = BTranslatorRoster::Default();
        BBitmapStream stream;
        BBitmap *result = NULL;
        if (roster->Translate(&file, NULL, NULL, &stream,
            B_TRANSLATOR_BITMAP) < B_OK)
            return NULL;
        stream.DetachBitmap(&result);
        return result;
    }

You can also initialize the class with a [`BBitmap`](BBitmap.md "BBitmap") and use it as input to [`BTranslatorRoster::Translate()`](BTranslatorRoster.md#BTranslatorRoster_Translate "Translate\(\)") to save it in a different format:
    
    
    void StoreTranslatorBitmap(BBitmap *bitmap, char *filename,
                               uint32 type)
    {
        BTranslatorRoster *roster = BTranslatorRoster::Default();
        BBitmapStream stream(bitmap); // init with contents of bitmap
        BFile file(filename, B_CREATE_FILE | B_WRITE_ONLY);
    
        roster->Translate(&stream, NULL, NULL, &file, type);
    }
