//
//  InputSDL.cpp
//  konvergo
//
//  Created by Lionel CHAZALLON on 16/10/2014.
//  Upgraded to SDL GameController API with haptic rumble support.
//

#include <QKeyEvent>
#include <QDebug>
#include "InputSDL.h"

#include <climits>
#include <cstdlib>

// Map SDL GameController buttons to semantic key names
static QString controllerButtonName(SDL_GameControllerButton button)
{
  switch (button)
  {
    case SDL_CONTROLLER_BUTTON_A:             return "KEY_GAMEPAD_A";
    case SDL_CONTROLLER_BUTTON_B:             return "KEY_GAMEPAD_B";
    case SDL_CONTROLLER_BUTTON_X:             return "KEY_GAMEPAD_X";
    case SDL_CONTROLLER_BUTTON_Y:             return "KEY_GAMEPAD_Y";
    case SDL_CONTROLLER_BUTTON_BACK:          return "KEY_GAMEPAD_BACK";
    case SDL_CONTROLLER_BUTTON_GUIDE:         return "KEY_GAMEPAD_GUIDE";
    case SDL_CONTROLLER_BUTTON_START:         return "KEY_GAMEPAD_START";
    case SDL_CONTROLLER_BUTTON_LEFTSTICK:     return "KEY_GAMEPAD_LEFTSTICK";
    case SDL_CONTROLLER_BUTTON_RIGHTSTICK:    return "KEY_GAMEPAD_RIGHTSTICK";
    case SDL_CONTROLLER_BUTTON_LEFTSHOULDER:  return "KEY_GAMEPAD_LEFTSHOULDER";
    case SDL_CONTROLLER_BUTTON_RIGHTSHOULDER: return "KEY_GAMEPAD_RIGHTSHOULDER";
    case SDL_CONTROLLER_BUTTON_DPAD_UP:       return "KEY_GAMEPAD_DPAD_UP";
    case SDL_CONTROLLER_BUTTON_DPAD_DOWN:     return "KEY_GAMEPAD_DPAD_DOWN";
    case SDL_CONTROLLER_BUTTON_DPAD_LEFT:     return "KEY_GAMEPAD_DPAD_LEFT";
    case SDL_CONTROLLER_BUTTON_DPAD_RIGHT:    return "KEY_GAMEPAD_DPAD_RIGHT";
    case SDL_CONTROLLER_BUTTON_MISC1:         return "KEY_GAMEPAD_MISC1";
    case SDL_CONTROLLER_BUTTON_PADDLE1:       return "KEY_GAMEPAD_PADDLE1";
    case SDL_CONTROLLER_BUTTON_PADDLE2:       return "KEY_GAMEPAD_PADDLE2";
    case SDL_CONTROLLER_BUTTON_PADDLE3:       return "KEY_GAMEPAD_PADDLE3";
    case SDL_CONTROLLER_BUTTON_PADDLE4:       return "KEY_GAMEPAD_PADDLE4";
    case SDL_CONTROLLER_BUTTON_TOUCHPAD:      return "KEY_GAMEPAD_TOUCHPAD";
    default:                                  return QString("KEY_GAMEPAD_BUTTON_%1").arg(button);
  }
}

// Map SDL GameController axes to semantic key names
static QString controllerAxisName(SDL_GameControllerAxis axis, bool positive)
{
  const char* dir = positive ? "POS" : "NEG";
  switch (axis)
  {
    case SDL_CONTROLLER_AXIS_LEFTX:        return QString("KEY_GAMEPAD_LEFTX_%1").arg(dir);
    case SDL_CONTROLLER_AXIS_LEFTY:        return QString("KEY_GAMEPAD_LEFTY_%1").arg(dir);
    case SDL_CONTROLLER_AXIS_RIGHTX:       return QString("KEY_GAMEPAD_RIGHTX_%1").arg(dir);
    case SDL_CONTROLLER_AXIS_RIGHTY:       return QString("KEY_GAMEPAD_RIGHTY_%1").arg(dir);
    case SDL_CONTROLLER_AXIS_TRIGGERLEFT:  return QString("KEY_GAMEPAD_TRIGGERLEFT");
    case SDL_CONTROLLER_AXIS_TRIGGERRIGHT: return QString("KEY_GAMEPAD_TRIGGERRIGHT");
    default:                               return QString("KEY_GAMEPAD_AXIS_%1_%2").arg(axis).arg(dir);
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool InputSDLWorker::initialize()
{
  close();

  // Init both GameController and Joystick subsystems
  if (SDL_Init(SDL_INIT_GAMECONTROLLER | SDL_INIT_JOYSTICK) < 0)
  {
    qCritical() << "SDL failed to initialize:" << SDL_GetError();
    return false;
  }

  SDL_SetHint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, "1");
  SDL_GameControllerEventState(SDL_ENABLE);
  SDL_JoystickEventState(SDL_ENABLE);

  refreshDeviceList();

  return true;
}

//////////////////////////////////////////////////////////////////////////////////////////////////
void InputSDLWorker::close()
{
  if (SDL_WasInit(SDL_INIT_GAMECONTROLLER | SDL_INIT_JOYSTICK))
  {
    qInfo() << "SDL is closing.";

    for (auto it = m_controllers.constBegin(); it != m_controllers.constEnd(); ++it)
      SDL_GameControllerClose(it.value());
    m_controllers.clear();

    for (auto it = m_joysticks.constBegin(); it != m_joysticks.constEnd(); ++it)
      SDL_JoystickClose(it.value());
    m_joysticks.clear();

    SDL_Event event;
    event.type = SDL_QUIT;
    SDL_PushEvent(&event);
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QString InputSDLWorker::nameForController(SDL_JoystickID id)
{
  if (m_controllers.contains(id))
    return SDL_GameControllerName(m_controllers[id]);
  return nameForJoystick(id);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QString InputSDLWorker::nameForJoystick(SDL_JoystickID id)
{
  if (m_joysticks.contains(id))
    return SDL_JoystickName(m_joysticks[id]);
  return "unknown device";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void InputSDLWorker::rumble(SDL_JoystickID id, Uint16 lowFreq, Uint16 highFreq, Uint32 durationMs)
{
  if (m_controllers.contains(id))
    SDL_GameControllerRumble(m_controllers[id], lowFreq, highFreq, durationMs);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void InputSDLWorker::run()
{
  QElapsedTimer polltimer;

  while (true)
  {
    SDL_Event event;
    polltimer.restart();

    while (SDL_PollEvent(&event))
    {
      switch (event.type)
      {
        case SDL_QUIT:
          SDL_Quit();
          return;

        // ---- GameController events (semantic buttons) ----
        case SDL_CONTROLLERBUTTONDOWN:
        {
          QString name = nameForController(event.cbutton.which);
          QString keycode = controllerButtonName((SDL_GameControllerButton)event.cbutton.button);
          emit receivedInput(name, keycode, InputBase::KeyDown);

          // Haptic feedback: strong on confirm, medium on back, light on nav
          if (event.cbutton.button == SDL_CONTROLLER_BUTTON_A)
            rumble(event.cbutton.which, 0xC000, 0xC000, 50);
          else if (event.cbutton.button == SDL_CONTROLLER_BUTTON_B)
            rumble(event.cbutton.which, 0x6000, 0, 30);
          else
            rumble(event.cbutton.which, 0x2000, 0, 20);
          break;
        }

        case SDL_CONTROLLERBUTTONUP:
        {
          QString name = nameForController(event.cbutton.which);
          QString keycode = controllerButtonName((SDL_GameControllerButton)event.cbutton.button);
          emit receivedInput(name, keycode, InputBase::KeyUp);
          break;
        }

        case SDL_CONTROLLERAXISMOTION:
        {
          auto axis = (SDL_GameControllerAxis)event.caxis.axis;
          auto value = event.caxis.value;
          auto id = event.caxis.which;
          QString name = nameForController(id);

          // Triggers are 0..32767, sticks are -32768..32767
          bool isTrigger = (axis == SDL_CONTROLLER_AXIS_TRIGGERLEFT ||
                            axis == SDL_CONTROLLER_AXIS_TRIGGERRIGHT);

          if (isTrigger)
          {
            if (value > 16384)
            {
              if (!m_axisState.contains((quint8)axis))
              {
                emit receivedInput(name, controllerAxisName(axis, true), InputBase::KeyDown);
                m_axisState.insert((quint8)axis, true);
              }
            }
            else if (value < 8000 && m_axisState.contains((quint8)axis))
            {
              emit receivedInput(name, controllerAxisName(axis, true), InputBase::KeyUp);
              m_axisState.remove((quint8)axis);
            }
          }
          else
          {
            // Stick axis with hysteresis
            if (std::abs(value) > 16384)
            {
              bool positive = value > 0;
              if (!m_axisState.contains((quint8)axis))
              {
                emit receivedInput(name, controllerAxisName(axis, positive), InputBase::KeyDown);
                m_axisState.insert((quint8)axis, positive);
              }
              else if (m_axisState.value((quint8)axis) != positive)
              {
                emit receivedInput(name, controllerAxisName(axis, m_axisState.value((quint8)axis)), InputBase::KeyUp);
                emit receivedInput(name, controllerAxisName(axis, positive), InputBase::KeyDown);
                m_axisState[(quint8)axis] = positive;
              }
            }
            else if (std::abs(value) < 10000 && m_axisState.contains((quint8)axis))
            {
              emit receivedInput(name, controllerAxisName(axis, m_axisState.value((quint8)axis)), InputBase::KeyUp);
              m_axisState.remove((quint8)axis);
            }
          }
          break;
        }

        case SDL_CONTROLLERDEVICEADDED:
        {
          qInfo() << "SDL detected controller was added.";
          refreshDeviceList();
          break;
        }

        case SDL_CONTROLLERDEVICEREMOVED:
        {
          qInfo() << "SDL detected controller was removed.";
          refreshDeviceList();
          break;
        }

        // ---- Fallback: raw joystick events for unmapped controllers ----
        case SDL_JOYBUTTONDOWN:
        {
          if (m_controllers.contains(event.jbutton.which))
            break;
          emit receivedInput(nameForJoystick(event.jbutton.which),
                             QString("KEY_BUTTON_%1").arg(event.jbutton.button),
                             InputBase::KeyDown);
          break;
        }

        case SDL_JOYBUTTONUP:
        {
          if (m_controllers.contains(event.jbutton.which))
            break;
          emit receivedInput(nameForJoystick(event.jbutton.which),
                             QString("KEY_BUTTON_%1").arg(event.jbutton.button),
                             InputBase::KeyUp);
          break;
        }

        case SDL_JOYDEVICEADDED:
        case SDL_JOYDEVICEREMOVED:
        {
          if (event.type == SDL_JOYDEVICEADDED && !SDL_IsGameController(event.jdevice.which))
          {
            qInfo() << "SDL detected unmapped joystick added.";
            refreshDeviceList();
          }
          break;
        }

        case SDL_JOYHATMOTION:
        {
          if (m_controllers.contains(event.jhat.which))
            break;

          QString hatName("KEY_HAT_");
          bool pressed = true;

          switch (event.jhat.value)
          {
            case SDL_HAT_CENTERED:
              if (!m_lastHat.isEmpty())
                hatName = m_lastHat;
              else
                hatName += "CENTERED";
              pressed = false;
              break;
            case SDL_HAT_UP:    hatName += "UP"; break;
            case SDL_HAT_DOWN:  hatName += "DOWN"; break;
            case SDL_HAT_RIGHT: hatName += "RIGHT"; break;
            case SDL_HAT_LEFT:  hatName += "LEFT"; break;
            default: break;
          }

          m_lastHat = hatName;
          emit receivedInput(nameForJoystick(event.jhat.which), hatName,
                             pressed ? InputBase::KeyDown : InputBase::KeyUp);
          break;
        }

        case SDL_JOYAXISMOTION:
        {
          if (m_controllers.contains(event.jaxis.which))
            break;

          auto axis = event.jaxis.axis;
          auto value = event.jaxis.value;

          if (std::abs(value) > 32768 / 2)
          {
            bool up = value < 0;
            if (!m_axisState.contains(axis))
            {
              emit receivedInput(nameForJoystick(event.jaxis.which),
                                 QString("KEY_AXIS_%1_%2").arg(axis).arg(up ? "UP" : "DOWN"),
                                 InputBase::KeyDown);
              m_axisState.insert(axis, up);
            }
            else if (m_axisState.value(axis) != up)
            {
              emit receivedInput(nameForJoystick(event.jaxis.which),
                                 QString("KEY_AXIS_%1_%2").arg(axis).arg(m_axisState.value(axis) ? "UP" : "DOWN"),
                                 InputBase::KeyUp);
              m_axisState.remove(axis);
            }
          }
          else if (std::abs(value) < 10000 && m_axisState.contains(axis))
          {
            emit receivedInput(nameForJoystick(event.jaxis.which),
                               QString("KEY_AXIS_%1_%2").arg(axis).arg(m_axisState.value(axis) ? "UP" : "DOWN"),
                               InputBase::KeyUp);
            m_axisState.remove(axis);
          }
          break;
        }

        default:
          break;
      }
    }

    // Fixed poll interval at ~60Hz
    if (polltimer.elapsed() < SDL_POLL_TIME)
      QThread::msleep(SDL_POLL_TIME - polltimer.elapsed());
  }
}

//////////////////////////////////////////////////////////////////////////////////////////////////
void InputSDLWorker::refreshDeviceList()
{
  // Close existing
  for (auto it = m_controllers.constBegin(); it != m_controllers.constEnd(); ++it)
    SDL_GameControllerClose(it.value());
  m_controllers.clear();

  for (auto it = m_joysticks.constBegin(); it != m_joysticks.constEnd(); ++it)
    SDL_JoystickClose(it.value());
  m_joysticks.clear();
  m_axisState.clear();

  int numJoysticks = SDL_NumJoysticks();
  qInfo() << "SDL found" << numJoysticks << "device(s)";

  for (int i = 0; i < numJoysticks; i++)
  {
    if (SDL_IsGameController(i))
    {
      SDL_GameController* controller = SDL_GameControllerOpen(i);
      if (controller)
      {
        SDL_Joystick* joy = SDL_GameControllerGetJoystick(controller);
        int instanceId = SDL_JoystickInstanceID(joy);
        qInfo() << "GameController #" << instanceId << "is" << SDL_GameControllerName(controller)
                << "(type:" << SDL_GameControllerGetType(controller) << ")";
        m_controllers[instanceId] = controller;
      }
    }
    else
    {
      SDL_Joystick* joystick = SDL_JoystickOpen(i);
      if (joystick)
      {
        int instanceId = SDL_JoystickInstanceID(joystick);
        qInfo() << "Joystick #" << instanceId << "is" << SDL_JoystickName(joystick)
                << "with" << SDL_JoystickNumButtons(joystick) << "buttons and"
                << SDL_JoystickNumAxes(joystick) << "axes (no GameController mapping)";
        m_joysticks[instanceId] = joystick;
      }
    }
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
InputSDL::InputSDL(QObject* parent) : InputBase(parent)
{
  m_thread = new QThread(this);
  m_sdlworker = new InputSDLWorker(nullptr);
  m_sdlworker->moveToThread(m_thread);

  connect(this, &InputSDL::run, m_sdlworker, &InputSDLWorker::run);
  connect(m_sdlworker, &InputSDLWorker::receivedInput, this, &InputBase::receivedInput);
  m_thread->start();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
InputSDL::~InputSDL()
{
  close();
  
  if (m_thread->isRunning())
  {
    m_thread->exit(0);
    m_thread->wait();
  }
}

//////////////////////////////////////////////////////////////////////////////////////////////////
bool InputSDL::initInput()
{
  bool retValue;
  QMetaObject::invokeMethod(m_sdlworker, "initialize", Qt::BlockingQueuedConnection,
                            Q_RETURN_ARG(bool, retValue));

  if (retValue)
    emit run();

  return retValue;
}

//////////////////////////////////////////////////////////////////////////////////////////////////
void InputSDL::close()
{
  QMetaObject::invokeMethod(m_sdlworker, "close", Qt::DirectConnection);
}
