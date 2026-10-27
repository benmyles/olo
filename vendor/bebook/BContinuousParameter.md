<!-- Classes And Methods - The Media Kit - BContinuousParameter -->

## BContinuousParameter

| Derived From:| [`BParameter`](BParameter.md "BParameter")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/ParameterWeb.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BContinuousParameter_Overview.md "BContinuousParameter")

  * [Constructor and Destructor](BContinuousParameter.md#BContinuousParameter_ConstructorDestructor)
  * [Member Functions](BContinuousParameter.md#BContinuousParameter_MemberFunctions)
  * [Constants](BContinuousParameter.md#BContinuousParameter_Constants)

  
  
* * *

### Constructor and Destructor

You never create or delete a `BContinuousParameter` object yourself. Use the appropriate [`BParameterGroup`](BParameterGroup.md "BParameterGroup") functions to create these objects and add them to groups.

* * *

### Member Functions

####  GetResponse(), SetResponse()

`void GetResponse([response](BContinuousParameter.md#response "response")* response,  
float* factor,  
float* offset);``void SetResponse([response](BContinuousParameter.md#response "response") response,  
float factor,  
float offset);`

`GetResponse()` returns in `response`, `factor`, and `offset` the response type, factor, and offset currently in effect for the `BContinuousParameter`.

`SetResponse()` sets these values.

The `response` describes how the `BContinuousParameter`'s value is displayed to me the user. For example, if your parameter is polynomial in scale, you may wish to record only the base value, and let the `BContinuousParameter` handle presenting the value to the user as a polynomial by raising the value to the power specified by `factor` before displaying the value on the screen.

The `offset` is added to the value after the transformation specified by response is computed, but before displaying the value.

If you wish the displayed value to be v2+1, you would use the following call:
    
    
    SetResponse(B_POLYNOMIAL, 2.0, 1.0);

Note

![Note](./images/admonitions/Info_32.png)

The mapping specified by these functions is only used for controls that display the current value of the control. The default system theme doesn't currently display this information. Since this information is only used for display purposes, you will still have to handle (as appropriate for your needs) mapping between the value of the control and the parameter value it represents.

For example, a `BContinuousParameter` that selects a frequency between 20 Hz and 20,000 Hz might use a range from 0 to 3, with an exponent base of 10 and a multiplication factor of 20. In this case, the parameter will receive values from 0.0 to 3.0, which the UI control knows to map to the values 20 through 20,000 for display purposes.

#### MaxValue()

`float MaxValue();`

Returns the maximum possible value the `BContinuousParameter` can take, which was specified when the control was created. The value is in the units specified when the control was created, such as "dB" or "Hz."

#### MinValue()

`float MinValue();`

Returns the minimum possible value the `BContinuousParameter` can take, which was specified when the object was created. The value is in the units specified when the object was created, such as "dB" or "Hz."

#### ValueStep()

`float ValueStep();`

Returns the granularity of values between `MinValue()` and `MaxValue()` supported by the hardware.

#### ValueType()

`virtual type_code ValueType();`

Return a type_code indicating the data type of the control's value; by default, this is `B_FLOAT_TYPE`.

* * *

### Constants

####  response

Declared in: `media/ParameterWeb.h`

Constant| Description  
---|---  
`B_LINEAR`.| For every unit of change in the parameter's value, the displayed value also changes by 1. The factor should be 1.Indicates that the user interface will display the parameter's value as-is.  
`B_POLYNOMIAL`.| The `factor` is a power to which the control's value is raised before being displayed.Displays the parameter's value raised to the factor power. For instance, if factor is 2, the displayed value would be the parameter's value squared.  
`B_EXPONENTIAL`.| The `factor` is the base; the displayed value is the factor raised to the power of the parameter's value.Displays the factor raised to the power of the parameter's value. If the parameter's value is 3 and the factor is 2, the displayed value would be 23 or 8.  
`B_LOGARITHMIC`.| The `factor` is the base; the displayed value is the base factor logarithm of the parameter's value.Displays the base factor logarithm of the parameter's value; if the factor is 10 and the parameter's value is 3, the displayed value would be log10(3).
