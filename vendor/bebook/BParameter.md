<!-- Classes And Methods - The Media Kit - BParameter -->

## BParameter

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/Parameter.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BParameter_Overview.md "BParameter")

  * [Constructor and Destructor](BParameter.md#BParameter_ConstructorDestructor)
  * [Member Functions](BParameter.md#BParameter_MemberFunctions)
  * [Constants](BParameter.md#BParameter_Constants)

  
  
* * *

### Constructor and Destructor

`BParameter` objects may only be created by using the appropriate functions in the [`BParameterGroup`](BParameterGroup.md "BParameterGroup") class.

* * *

### Member Functions

####  AddInput(), AddOutput()

`status_t AddInput(BParameter* input);``status_t AddOutput(BParameter* output);`

`AddInput()` adds the `BParameter` specified by `input` to the set of `BParameter`s that transmit data into the `BParameter`. The specified input parameter's `AddOutput()` function is called to let it know that it's sending data to this parameter.

`AddOutput()` adds the `BParameter` specified by `output` to the set of `BParameter`s to which the `BParameter` outputs data. The specified `output` parameter's `AddInput()` function is called to let it know that it's receiving data from this parameter.

Return Code| Description  
---|---  
`B_OK`| The item was successfully added to the list.  
`B_ERROR`| The item couldn't be added to the list.  
  
####  CountChannels(), SetChannelCount()

`int32 CountChannels();``void SetChannelCount(int32 numChannels);`

Some `BParameter`s have more than one channel of the same type. For example, a stereo volume parameter with independent left and right gain would have two channels, while a single parameter value that affects both the left and right by the same amount would have only one channel. `CountChannels()` returns the number of channels manipulated by the `BParameter`.

The default is one channel; if your node provides more channels, call `SetChannelCount()` after creating the `BParameter`. `SetChannelCount()` specifies the number of channels the parameter manipulates.

####  CountInputs(), CountOutputs()

`int32 CountInputs();``int32 CountOutputs();`

These two functions return the number of inputs or outputs currently connected to the `BParameter`. The number of inputs or outputs may be different from the number of channels, since an input or output might be comprised of multiple channels (a single stereo sound input or output might consist of a left channel and a right channel, for example).

####  GetValue(), SetValue()

`status_t GetValue(void* buffer,  
size_t* ioSize,  
bigtime_t* lastChange);``status_t SetValue(const void* buffer,  
size_t size,  
bigtime_t changeWhen);`

`GetValue()` returns the current value of the `BParameter`. On entry, `buffer` points to `ioSize` bytes of memory, indicating where the current value of the `BParameter` should be stored. On return, `ioSize` indicates the actual number of bytes returned in the buffer, and `lastChange` indicates the time at which the `BParameter`'s value last changed.

`SetValue()` changes the value of the `BParameter`. The new value, which is `size` bytes long, is obtained from the specified `buffer`. The change occurs at the performance time specified by `changeWhen`.

In either case, if the parameter is a multichannel parameter, the buffer should be large enough to contain the values for all the channels, and `ioSize` or `size` should indicate the total size of the buffer.

These calls simply dispatch to the [`BControllable::GetParameterValue()`](BControllable.md#BControllable_GetParameterValue "GetParameterValue\(\), SetParameterValue\(\)") and [`BControllable::SetParameterValue()`](BControllable.md#BControllable_SetParameterValue) functions.

Return Code| Description  
---|---  
`B_OK`| No errors.  
`B_NO_MEMORY`| The buffer is too small (`GetValue()`).  
`B_BAD_VALUE`| The parameter doesn't have a value (`GetValue()`).  
Port errors.| See [Ports](TheKernelKit_Ports_Overview.md "Ports") in the [The Kernel Kit](TheKernelKit_Overview.md "The Kernel Kit")  
  
#### Group()

`BParameterGroup* Group() const;`

Returns the [`BParameterGroup`](BParameterGroup.md "BParameterGroup") that most directly contains the `BParameter`.

#### ID()

`int32 ID() const;`

Returns the `BParameter`'s ID number. This ID is used by the node to route value change requests to the right place. This value should be unique within the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") containing the `BParameter`.

####  InputAt(), OutputAt()

`BParameter* InputAt(int32 index);``BParameter* OutputAt(int32 index);`

`InputAt()` returns the `BParameter` that feeds data into the input number specified by `index`, which can range from zero to [`CountInputs()`](BParameter.md#BParameter_CountInputs "CountInputs\(\), CountOutputs\(\)")-1.

`OutputAt()` returns the `BParameter` that receives data from the output number specified by `index`, which can range from zero to [`CountOutputs()`](BParameter.md#BParameter_CountInputs "CountInputs\(\), CountOutputs\(\)")-1.

Both functions return `NULL` if an invalid `index` is specified.

#### Kind()

`const char* Kind() const;`

Returns the `BParameter`'s kind. The kind isn't necessarily user-readable, but it can be used to figure out what kind of [`BControl`](BControl.md "BControl") to create to visually represent the configuration option provided by the `BParameter`.

See the [`BParameterGroup::MakeNullParameter()`](BParameterGroup.md#BParameterGroup_MakeNullParameter "MakeNullParameter\(\)"), [`BParameterGroup::MakeDiscreteParameter()`](BParameterGroup.md#BParameterGroup_MakeDiscreteParameter "MakeDiscreteParameter\(\)"), and [`BParameterGroup::MakeContinuousParameter()`](BParameterGroup.md#BParameterGroup_MakeContinuousParameter "MakeContinuousParameter\(\)") functions for more detailed information about the available kinds.

####  MediaType(), SetMediaType()

`media_type MediaType();``void SetMediaType(media_type type);`

`MediaType()` returns the type of media data that flows through the `BParameter`.

`SetMediaType()` changes the recorded type of media data. The default, if it hasn't been changed, is `B_MEDIA_NO_TYPE`.

The [media_type](TheMediaKit_Constants.md#Constants_media_type "media_type") of a `BParameter` is used to inform interested parties as to the format of the data; it's an informational setting, and doesn't alter the data flowing through the `BParameter` in any way.

#### Name()

`const char* Name() const;`

Returns the `BParameter`'s name, which is suitable for display to the user (for example, it can be used as the label in a user interface object).

#### Type()

`media_parameter_type Type() const;`

Returns the kind of parameter the object represents.

Constant| Description  
---|---  
`B_NULL_PARAMETER`|  The `BParameter` is a [`BNullParameter`](BNullParameter.md "BNullParameter").  
`B_DISCRETE_PARAMETER`| The `BParameter` is a [`BDiscreteParameter`](BDiscreteParameter.md "BDiscreteParameter").  
`B_CONTINUOUS_PARAMETER`| The `BParameter` is a [`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter").  
  
#### Unit()

`const char* Unit() const;`

Returns the unit of measurement used by the `BParameter`. This might be, for example, "dB," "kHz," or "fps." It should be human-readable, as it may be displayed in a user interface.

#### ValueType()

`virtual type_code ValueType() = 0;`

Returns a type code indicating the type of data type of the `BParameter`'s value. This is usually `B_INT32_TYPE` for selectors or `B_FLOAT_TYPE` for sliders.

#### Web()

`[`BParameterWeb`](BParameterWeb.md "BParameterWeb")* Web() const;`

Returns the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that contains the `BParameter`.

* * *

### Constants

#### Parameter Kinds

Declared in: `media/ParameterWeb.h`

Constant| Description  
---|---  
`B_MASTER_GAIN`| The parameter represents the main volume control.  
`B_GAIN`| The parameter represents a gain control.  
`B_BALANCE`| The parameter represents a balance control.  
`B_FREQUENCY`| The parameter represents a frequency, like a radio tuner.  
`B_LEVEL`| The parameter represents a level, as in EQ and effects.  
`B_SHUTTLE_SPEED`| The parameter represents playback speed. A value of 1.0 indicates normal speed; less than 1.0 is slower, greater than 1.0 is faster.  
`B_CROSSFADE`| The parameter indicates a crossfade for mixing audio or video; 0 indicates that the the first of a pair of streams should be presented, 100 indicates that the other should be presented, and values in between indicate the degree of mixing that should occur.  
`B_COMPRESSION`| The parameter indicates compression ratio; the value is 0 for no compression, 99 for 100:1 compression.  
`B_QUALITY`| The parameter indicates quality level; the value is 0 for maximum compression, 100 for no compression.  
`B_BITRATE`| The parameter indicates bit rate in bits per second.  
`B_GOP_SIZE`| The parameter indicates a "group of pictures" such as how many frames apart a keyframe should be inserted into a video stream.  
`B_MUTE`| The parameter represents a mute control; a value of 0 passes data through unchanged, while a value of 1 mutes the data.  
`B_ENABLE`| The parameter represents an enable toggle; a value of 0 disables the function while a value of 1 enables it.  
`B_INPUT_MUX`| The parameter represents an input MUX. The value specifies which input should pass through.  
`B_OUTPUT_MUX`|  The parameter represents an output MUX; the value specifies which output should receive the incoming data.  
`B_TUNER_CHANNEL`| The parameter represents a channel tuner (like a TV channel); the value indicates the channel number.  
`B_TRACK`| The parameter's value indicates a track number.  
`B_RECSTATE`| The parameter indicates whether the node is silent (0), playing (1), or recording (2).  
`B_SHUTTLE_MODE`| The parameter indicates performance mode: -1 for backwards, 0 for stop, 1 for playing, and 2 for paused.  
`B_RESOLUTION`| The parameter indicates video or audio resolution.  
`B_COLOR_SPACE`| The parameter indicates video color space.  
`B_FRAME_RATE`| The parameter represents a selector for picking frame rate.  
`B_VIDEO_FORMAT`| The parameter represents a selector for choosing a video format.The `B_VIDEO_FORMAT` kind has specific video format values associated with it:| 1| NTSC-M  
---|---  
2| NTSC-J  
3| PAL-BDGHI  
4| PAL-M  
5| PAL-N  
6| SECAM  
7| MPEG-1  
8| MPEG-2  
`B_WEB_PHYSICAL_INPUT`| The parameter represents a physical input (such as a microphone jack or line input jack).  
`B_WEB_PHYSICAL_OUTPUT`| The parameter represents a physical output (such as a line output jack or headphone jack).  
`B_WEB_LOGICAL_INPUT`| The parameter represents a point at which bits of data are transferred between the computer and the A/V input hardware.  
`B_WEB_LOGICAL_OUTPUT`| The parameter represents a point at which bits of data are transferred between the computer and the A/V output hardware.  
`B_WEB_ADC_CONVERTER`| The parameter represents an analog to digital converter.  
`B_WEB_DAC_CONVERTER`| The parameter represents a digital to analog converter.  
`B_WEB_BUFFER_INPUT`| The parameter represents a [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input").  
`B_WEB_BUFFER_OUTPUT`| The parameter represents a [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output").  
`B_SIMPLE_TRANSPORT`| A discrete parameter indicating one of five states: rewinding, stopped, playing, paused, and fast-forwarding.  
`B_GENERIC`| The parameter's kind isn't one of the above.  
  
Indicates the kind of parameter represented by a `BParameter` object.

#### media_parameter_type

Declared in: `media/ParameterWeb.h`

Constant| Description  
---|---  
`B_NULL_PARAMETER`|  The `BParameter` is a [`BNullParameter`](BNullParameter.md "BNullParameter").  
`B_DISCRETE_PARAMETER`| The `BParameter` is a [`BDiscreteParameter`](BDiscreteParameter.md "BDiscreteParameter").  
`B_CONTINUOUS_PARAMETER`| The `BParameter` is a [`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter").  
  
These are the possible parameter types, which are returned by the [`Type()`](BParameter.md#BParameter_Type "Type\(\)") function. They indicate what subclass of `BParameter` the object is.

Note

![Note](./images/admonitions/Info_32.png)

Keep in mind that because these constants are members of the `BParameter` class, if you need to reference them from outside a BParameter, you need to preface the reference with "BParameter::", such as `BParameter`::`B_NULL_PARAMETER`.
