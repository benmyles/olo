<!-- System Overview - The Application Kit - Introduction -->

## Introduction

The Application Kit is the starting point for all applications. Its classes establish an application as an identifiable entity; one that can cooperate and communicate with other applications.

The Application Kit is divided into these topics:

[`BApplication`](BApplication.md "BApplication").
    

All but the simplest of applications must have one (and only one) [`BApplication`](BApplication.md "BApplication") object. This object is typically an instance of a [`BApplication`](BApplication.md "BApplication") subclass that you create especially for your application. The [`BApplication`](BApplication.md "BApplication") object makes a connection to the App Server and runs the application's main message loop.

[Messaging](TheApplicationKit_Messaging.md "Messaging").
    

The kit provides a messaging service that lets threads talk to each other. This service can deliver messages within your own application, or from one application to another. It's also used by the system to deliver user event messages (key clicks, mouse moves) to your application. Most of the Application Kit's classes are involved in the messaging system.

[Scripting](TheApplicationKit_Scripting.md "Scripting").
    

The objects that you create can be controlled by commands issued from other applications.

[`BRoster`](BRoster.md "BRoster").
    

The [`BRoster`](BRoster.md "BRoster") object keeps track of all running applications. It can identify applications, launch them, and provide the information needed to set up communications with them.

[`BClipboard`](BClipboard.md "BClipboard").
    

The [`BClipboard`](BClipboard.md "BClipboard") object provides an interface to the clipboard where cut and copied data can be stored, and from which it can be pasted.

[`BCursor`](BCursor.md "BCursor").
    

You use [`BCursor`](BCursor.md "BCursor") objects to represent distinct cursors. Functions defined by [`BApplication`](BApplication.md "BApplication") and [`BView`](BView.md "BView") let you assign your cursors to your entire application, or to individual views.
